//! Streams canonical sites and attaches the shared hybrid meshes to the planet.
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use bevy::{
    camera::{
        primitives::{Frustum, Sphere},
        visibility::VisibilityRange,
    },
    light::CascadeShadowConfig,
    math::DVec3,
    mesh::MeshTag,
    prelude::*,
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
};
use igor_renderer::Detail;
use world_core::{
    planet::RADIUS,
    runtime::{GridTransform, PlanetRoot, ReferenceFrames, WorldCamera},
    SurfaceHeights,
};

use super::{
    hybrid::{HybridModels, Model},
    lod::{VegetationSettings, View},
    placement::{NodeKey, PatchKey, Site},
    within_horizon, CoconutPalm, ForestClearing, Species,
};

const MAX_NODES: usize = 112;
const NODE_ALLOWANCE: f32 = 480.0;
const DISCOVERY_INTERVAL: f64 = 0.16;
const PLACEMENT_JOBS: usize = 12;
const SPAWNS_PER_FRAME: usize = 384;
const CELL_METERS: f64 = 32.0;

pub(super) type Sites = Arc<[Site]>;

struct Leaf {
    key: PatchKey,
    anchor: DVec3,
    sites: Option<Sites>,
    placement: Option<Task<Sites>>,
    trees: Vec<Tree>,
}

struct Node {
    anchor: DVec3,
    leaves: Vec<Leaf>,
}

impl Node {
    fn despawn(&self, commands: &mut Commands) {
        for tree in self.leaves.iter().flat_map(|leaf| &leaf.trees) {
            tree.despawn(commands);
        }
    }
}

struct Tree {
    parts: Option<[Option<Entity>; 2]>,
    center: DVec3,
    radius: f32,
    cell_center: DVec3,
    rank: f32,
    detail: Option<Detail>,
    phase: u32,
}

impl Tree {
    fn new(site: Site, model: &Model, index: usize, key: PatchKey, anchor: DVec3) -> Self {
        let up = anchor.normalize();
        let (east, north) = super::tangent_basis(up);
        let offset = site.position - anchor;
        let cell = [offset.dot(east), offset.dot(north)].map(|v| (v / CELL_METERS).floor() as i32);
        let cell_center = anchor
            + east * ((f64::from(cell[0]) + 0.5) * CELL_METERS)
            + north * ((f64::from(cell[1]) + 0.5) * CELL_METERS)
            + up * offset.dot(up);
        let id = (key.0 as u32).wrapping_mul(0x9e3779b9)
            ^ (key.1 as u32).wrapping_mul(0x85ebca6b)
            ^ index as u32;
        let center = site.position
            + (site.rotation() * Vec3::from(model.bounds.center) * site.scale).as_dvec3();
        Self {
            parts: None,
            center,
            radius: model.bounds.half_extents.length() * site.scale,
            cell_center,
            rank: igor_renderer::rank(id, cell),
            detail: None,
            phase: 0,
        }
    }

    fn despawn(&self, commands: &mut Commands) {
        for entity in self.parts.iter().flatten().flatten() {
            commands.entity(*entity).despawn();
        }
    }
}

#[derive(Resource, Default)]
pub(super) struct ForestStream {
    construction_revision: u64,
    nodes: HashMap<NodeKey, Node>,
    order: Vec<NodeKey>,
    next_plan: f64,
    last_eye: Option<DVec3>,
    last_view: DVec3,
    settings_extent: f32,
    next_report: f64,
    pub stats: StreamStats,
}

#[derive(Default)]
pub(super) struct StreamStats {
    pub nodes: usize,
    pub unsampled: usize,
    pub jobs: usize,
    pub levels: [usize; 3],
    pub blends: usize,
    pub triangles: usize,
}

impl ForestStream {
    pub fn cached_sites(&self) -> HashMap<PatchKey, Sites> {
        self.nodes
            .values()
            .flat_map(|node| &node.leaves)
            .filter_map(|leaf| Some((leaf.key, leaf.sites.as_ref()?.clone())))
            .collect()
    }

    pub fn sites(&self, key: PatchKey) -> Option<&[Site]> {
        self.nodes
            .get(&key.parent())?
            .leaves
            .iter()
            .find(|leaf| leaf.key == key)?
            .sites
            .as_deref()
    }

    fn plan(
        &mut self,
        commands: &mut Commands,
        eye: DVec3,
        up: DVec3,
        forward: DVec3,
        now: f64,
        extent: f32,
        heights: &SurfaceHeights,
    ) {
        let moved = self
            .last_eye
            .is_none_or(|last| last.distance_squared(eye) > 115.0_f64.powi(2));
        let turned = self.last_view.dot(forward) < 0.63;
        if self.settings_extent == extent && (!moved && !turned || now < self.next_plan && !moved) {
            return;
        }
        self.next_plan = now + DISCOVERY_INTERVAL;
        self.last_eye = Some(eye);
        self.last_view = forward;
        self.settings_extent = extent;

        let altitude = (eye.length() - RADIUS) - heights.sample(up).unwrap_or(0.0);
        let reach = f64::from(extent + NODE_ALLOWANCE);
        let mut candidates = Vec::new();
        if altitude < reach {
            for key in NodeKey::around(up, reach) {
                let location = self
                    .nodes
                    .get(&key)
                    .map_or_else(|| anchor(heights, key.center()), |node| node.anchor);
                let vector = location - eye;
                let distance = vector.length();
                if distance <= reach || (self.nodes.contains_key(&key) && distance < reach + 180.0)
                {
                    // Request the whole disk. Favor nearer, forward tiles when crossing into new terrain.
                    let priority =
                        distance + 135.0 * (1.0 - vector.normalize_or_zero().dot(forward));
                    candidates.push((priority, key, location));
                }
            }
        }
        candidates.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        candidates.truncate(MAX_NODES);
        let wanted: HashSet<_> = candidates.iter().map(|&(_, key, _)| key).collect();
        self.nodes.retain(|key, node| {
            if wanted.contains(key) {
                return true;
            }
            node.despawn(commands);
            false
        });
        self.order.clear();
        for (_, key, location) in candidates {
            self.order.push(key);
            self.nodes.entry(key).or_insert_with(|| {
                let leaves = key
                    .leaves()
                    .into_iter()
                    .map(|patch| Leaf {
                        key: patch,
                        anchor: anchor(heights, patch.center()),
                        sites: None,
                        placement: None,
                        trees: Vec::new(),
                    })
                    .collect();
                Node {
                    anchor: location,
                    leaves,
                }
            });
        }
    }

    fn allocate_sites(
        &mut self,
        heights: &SurfaceHeights,
        eye: DVec3,
        clearing: Option<&ForestClearing>,
    ) {
        let mut pending = 0;
        for leaf in self.nodes.values_mut().flat_map(|node| &mut node.leaves) {
            let ready = leaf
                .placement
                .as_mut()
                .and_then(|task| block_on(poll_once(task)));
            if let Some(sites) = ready {
                leaf.placement = None;
                leaf.sites = Some(sites);
            }
            pending += usize::from(leaf.placement.is_some());
        }
        let mut budget = PLACEMENT_JOBS.saturating_sub(pending);
        let pool = AsyncComputeTaskPool::get();
        for key in &self.order {
            if budget == 0 {
                break;
            }
            let node = self.nodes.get_mut(key).unwrap();
            let mut leaves: Vec<_> = node
                .leaves
                .iter_mut()
                .filter(|leaf| leaf.sites.is_none() && leaf.placement.is_none())
                .collect();
            leaves.sort_unstable_by(|a, b| {
                eye.distance_squared(a.anchor)
                    .total_cmp(&eye.distance_squared(b.anchor))
                    .then(a.key.cmp(&b.key))
            });
            for leaf in leaves.into_iter().take(budget) {
                let key = leaf.key;
                let heights = heights.clone();
                let clearing = clearing.copied();
                leaf.placement = Some(pool.spawn(async move {
                    key.sample_sites(&heights, clearing, 1).collect::<Sites>()
                }));
                budget -= 1;
            }
        }
    }
}
fn anchor(heights: &SurfaceHeights, direction: DVec3) -> DVec3 {
    direction * (RADIUS + heights.sample(direction).unwrap_or(0.0))
}

pub(super) fn stream(
    mut commands: Commands,
    settings: Res<VegetationSettings>,
    time: Res<Time>,
    root: Res<PlanetRoot>,
    heights: Res<SurfaceHeights>,
    clearing: Option<Res<ForestClearing>>,
    cameras: Query<(Entity, GridTransform, &Camera, &Projection), With<WorldCamera>>,
    lights: Query<(&DirectionalLight, &Transform, &CascadeShadowConfig), Without<WorldCamera>>,
    frames: ReferenceFrames,
    models: Res<HybridModels>,
    mut forest: ResMut<ForestStream>,
) {
    let Ok((camera_entity, camera, view, projection)) = cameras.single() else {
        return;
    };
    let Some(planet_frame) = frames.parent_frame(camera_entity) else {
        return;
    };
    let eye = planet_frame.grid_position_double(&camera.cell, &camera.transform);
    let Some(up) = eye.try_normalize() else {
        return;
    };
    let forest = &mut *forest;
    if forest.construction_revision != heights.construction_revision() {
        forest.construction_revision = heights.construction_revision();
        for node in std::mem::take(&mut forest.nodes).into_values() {
            node.despawn(&mut commands);
        }
        forest.order.clear();
        forest.last_eye = None;
    }
    let forward = (camera.transform.rotation * Vec3::NEG_Z).as_dvec3();
    let band = settings.forest_band();
    let preload = settings.preload_margin.clamp(90.0, 420.0);
    let now = time.elapsed_secs_f64();
    forest.plan(
        &mut commands,
        eye,
        up,
        forward,
        now,
        band.y + preload,
        &heights,
    );
    forest.allocate_sites(&heights, eye, clearing.as_deref());

    let mut stats = StreamStats {
        nodes: forest.nodes.len(),
        ..default()
    };
    let frustum = projection.compute_frustum(&GlobalTransform::from(Transform::from_rotation(
        camera.transform.rotation,
    )));
    let shadows: Vec<_> = lights
        .iter()
        .filter(|(light, _, _)| light.shadow_maps_enabled)
        .filter_map(|(_, transform, config)| {
            Some((*transform.back(), config.bounds.last().copied()?))
        })
        .collect();
    let view = View::new(view, projection);
    let selection = settings.selection();
    let elapsed = time.delta_secs().min(0.075);
    let range = VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: band.x..band.y,
        use_aabb: false,
    };
    let mut budget = SPAWNS_PER_FRAME;
    for key in &forest.order {
        let node = forest.nodes.get_mut(key).unwrap();
        for leaf in &mut node.leaves {
            stats.jobs += usize::from(leaf.placement.is_some());
            let Some(sites) = &leaf.sites else {
                stats.unsampled += 1;
                continue;
            };
            if !models.ready() {
                continue;
            }
            let leaf_distance = eye.distance(leaf.anchor);
            if leaf.trees.is_empty() && leaf_distance > f64::from(band.y) + 240.0 {
                continue;
            }
            if leaf.trees.is_empty() {
                leaf.trees = sites
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(i, site)| Tree::new(site, models.model(site), i, leaf.key, leaf.anchor))
                    .collect();
            }
            for (&site, tree) in sites.iter().zip(&mut leaf.trees) {
                let offset = tree.center - eye;
                let distance = offset.length() as f32;
                let resident = distance <= band.y + tree.radius + 12.0
                    && potentially_visible(
                        &frustum,
                        offset.as_vec3(),
                        distance,
                        tree.radius,
                        tree.parts.is_some(),
                        &shadows,
                    )
                    && within_horizon(eye, tree.center, f64::from(tree.radius));
                if !resident {
                    if tree.parts.is_some() {
                        tree.despawn(&mut commands);
                        tree.parts = None;
                        tree.detail = None;
                    }
                    continue;
                }
                if tree.parts.is_none() && budget == 0 {
                    continue;
                }
                let pixels = view.pixels(2.0 * tree.radius, distance - tree.radius);
                let cell_radius =
                    CELL_METERS as f32 * std::f32::consts::FRAC_1_SQRT_2 + tree.radius;
                let cell_pixels = view.pixels(
                    2.0 * cell_radius,
                    eye.distance(tree.cell_center) as f32 - cell_radius,
                );
                let phase =
                    selection.select(&mut tree.detail, pixels, cell_pixels, tree.rank, elapsed);
                let stage = phase.floor() as usize;
                let tag = (phase.fract() * 65535.0).round() as u32;
                let packed = tag | ((stage as u32) << 16);
                let model = models.model(site);
                let parts = [model.wood.as_ref(), Some(&model.leaves)];
                stats.levels[stage] += 1;
                stats.blends += usize::from(tag != 0);
                stats.triangles += parts
                    .iter()
                    .flatten()
                    .map(|part| part.triangles[stage])
                    .sum::<usize>();
                if let Some(entities) = tree.parts {
                    if packed != tree.phase || settings.is_changed() {
                        for (entity, part) in entities.into_iter().zip(parts) {
                            let Some((id, part)) = entity.zip(part) else {
                                continue;
                            };
                            let mut entity = commands.entity(id);
                            if packed != tree.phase {
                                entity.insert(MeshTag::new(tag));
                                if stage != (tree.phase >> 16) as usize {
                                    entity.insert(Mesh3d(part.meshes[stage].clone()));
                                }
                            }
                            if settings.is_changed() {
                                entity.insert(range.clone());
                            }
                        }
                    }
                } else {
                    let (cell, translation) = planet_frame.translation_to_grid(site.position);
                    let transform = Transform::from_translation(translation)
                        .with_rotation(site.rotation())
                        .with_scale(Vec3::splat(site.scale));
                    let mut ids = [None; 2];
                    for (i, part) in parts.into_iter().enumerate() {
                        let Some(part) = part else { continue };
                        let mut entity = root.spawn(
                            &mut commands,
                            (
                                transform,
                                cell,
                                part.bounds,
                                range.clone(),
                                MeshTag::new(tag),
                                Mesh3d(part.meshes[stage].clone()),
                                MeshMaterial3d(part.material.clone()),
                            ),
                        );
                        if i == 1 {
                            let name = super::library::asset(site.species, site.variant)
                                .map_or_else(
                                    || {
                                        if site.species == Species::Jungle && site.variant == 1 {
                                            "tree_part9/tree"
                                        } else {
                                            "Forest tree"
                                        }
                                    },
                                    |asset| asset.path,
                                );
                            entity.insert(Name::new(name));
                            if site.species == Species::Palm && site.variant == 0 {
                                entity.insert(CoconutPalm);
                            }
                        }
                        ids[i] = Some(entity.id());
                    }
                    tree.parts = Some(ids);
                    budget -= 1;
                }
                tree.phase = packed;
            }
        }
    }
    if models.ready() && now >= forest.next_report {
        forest.next_report = now + 5.0;
        info!("Hybrid forest: LOD={:?} transitions={} resident_triangles={} nodes={} placement_jobs={}",
              stats.levels, stats.blends, stats.triangles, stats.nodes, stats.jobs);
    }
    forest.stats = stats;
}

// Use camera-relative centers to keep small crowns precise across planet cells.
// Keep the near ring and trees whose directional shadow can reach a cascade.
fn potentially_visible(
    frustum: &Frustum,
    offset: Vec3,
    distance: f32,
    radius: f32,
    was_resident: bool,
    shadows: &[(Vec3, f32)],
) -> bool {
    if distance < 180.0 + radius {
        return true;
    }
    let margin = if was_resident { 0.22 } else { 0.15 };
    let sphere = Sphere {
        center: offset.into(),
        radius: radius + distance * margin,
    };
    frustum.intersects_sphere(&sphere, false)
        || shadows.iter().any(|&(sun, reach)| {
            let shadow_offset = offset - sun * offset.dot(sun).max(0.0);
            shadow_offset.length_squared() < (reach * 1.42 + radius).powi(2)
        })
}

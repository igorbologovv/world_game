//! Streams canonical sites, forest regions, and gameplay markers.
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use bevy::{
    math::DVec3,
    prelude::*,
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
};
use world_core::{
    planet::RADIUS,
    runtime::{GridTransform, PlanetRoot, ReferenceFrames, WorldCamera},
    SurfaceHeights,
};

use super::{
    hybrid::{ModelLoading, RenderChunk},
    lod::VegetationSettings,
    placement::{NodeKey, PatchKey, Site},
    within_horizon, CoconutPalm, ForestClearing, Species,
};

const MAX_NODES: usize = 112;
const NODE_ALLOWANCE: f32 = 480.0;
const DISCOVERY_INTERVAL: f64 = 0.16;
const PLACEMENT_JOBS: usize = 12;
const REGION_JOBS: usize = 12;
const PALM_MARKER_RANGE: f64 = 320.0;

pub(super) type Sites = Arc<[Site]>;

struct Leaf {
    key: PatchKey,
    anchor: DVec3,
    sites: Option<Sites>,
    placement: Option<Task<Sites>>,
    region: RegionState,
    palms: Vec<Entity>,
}

#[derive(Default)]
enum RegionState {
    #[default]
    Waiting,
    Building(Task<Option<RenderChunk>>),
    Ready(Option<Arc<RenderChunk>>),
}

struct Node {
    anchor: DVec3,
    leaves: Vec<Leaf>,
}

impl Node {
    fn despawn(&self, commands: &mut Commands) {
        for &marker in self.leaves.iter().flat_map(|leaf| &leaf.palms) {
            commands.entity(marker).despawn();
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
    pub stats: StreamStats,
}

#[derive(Default)]
pub(super) struct StreamStats {
    pub nodes: usize,
    pub unsampled: usize,
    pub jobs: usize,
}

impl ForestStream {
    pub fn chunks(&self) -> impl Iterator<Item = (PatchKey, Arc<RenderChunk>)> + '_ {
        self.nodes
            .values()
            .flat_map(|node| &node.leaves)
            .filter_map(|leaf| match &leaf.region {
                RegionState::Ready(Some(chunk)) => Some((leaf.key, Arc::clone(chunk))),
                _ => None,
            })
    }

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
                        region: RegionState::Waiting,
                        palms: Vec::new(),
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
    cameras: Query<(Entity, GridTransform), With<WorldCamera>>,
    frames: ReferenceFrames,
    models: Option<Res<ModelLoading>>,
    lights: Query<(&DirectionalLight, &GlobalTransform)>,
    mut forest: ResMut<ForestStream>,
) {
    let Ok((camera_entity, camera)) = cameras.single() else {
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
    forest.plan(
        &mut commands,
        eye,
        up,
        forward,
        time.elapsed_secs_f64(),
        band.y + preload,
        &heights,
    );
    forest.allocate_sites(&heights, eye, clearing.as_deref());

    let mut stats = StreamStats {
        nodes: forest.nodes.len(),
        ..default()
    };
    let models = models.as_ref().and_then(|loading| loading.models.as_ref());
    let mut building = 0;
    let mut waiting = Vec::new();
    for leaf in forest.nodes.values_mut().flat_map(|node| &mut node.leaves) {
        stats.jobs += usize::from(leaf.placement.is_some());
        let Some(sites) = &leaf.sites else {
            stats.unsampled += 1;
            continue;
        };
        if models.is_none() || sites.is_empty() {
            continue;
        }
        if let RegionState::Building(task) = &mut leaf.region {
            if let Some(region) = block_on(poll_once(task)) {
                leaf.region = RegionState::Ready(region.map(Arc::new));
            } else {
                building += 1;
            }
        }
        let distance = eye.distance(leaf.anchor);
        update_palms(&mut commands, &root, planet_frame, leaf, distance);
        if matches!(leaf.region, RegionState::Waiting)
            && distance <= f64::from(band.y + preload)
            && within_horizon(eye, leaf.anchor, 60.0)
        {
            waiting.push((distance, leaf));
        }
    }
    if let Some(models) = models {
        waiting.sort_unstable_by(|(dist_a, a), (dist_b, b)| {
            dist_a.total_cmp(dist_b).then(a.key.cmp(&b.key))
        });
        let pool = AsyncComputeTaskPool::get();
        let sun = super::hybrid::sun_direction(&lights);
        for (_, leaf) in waiting
            .into_iter()
            .take(REGION_JOBS.saturating_sub(building))
        {
            let models = Arc::clone(models);
            let sites = Arc::clone(leaf.sites.as_ref().unwrap());
            let anchor = leaf.anchor;
            leaf.region = RegionState::Building(
                pool.spawn(async move { RenderChunk::build(&models, anchor, &sites, sun) }),
            );
        }
    }
    forest.stats = stats;
}

fn update_palms(
    commands: &mut Commands,
    root: &PlanetRoot,
    frame: &world_core::runtime::ReferenceFrame,
    leaf: &mut Leaf,
    distance: f64,
) {
    if distance > PALM_MARKER_RANGE {
        for marker in leaf.palms.drain(..) {
            commands.entity(marker).despawn();
        }
    } else if leaf.palms.is_empty() {
        for site in leaf
            .sites
            .iter()
            .flat_map(|sites| sites.iter())
            .filter(|site| site.species == Species::Palm && site.variant == 0)
        {
            let (cell, translation) = frame.translation_to_grid(site.position);
            let transform = Transform::from_translation(translation)
                .with_rotation(site.rotation())
                .with_scale(Vec3::splat(site.scale));
            leaf.palms
                .push(root.spawn(commands, (cell, transform, CoconutPalm)).id());
        }
    }
}

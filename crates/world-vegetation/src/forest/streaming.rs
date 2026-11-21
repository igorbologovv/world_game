//! Bounded chunk discovery and placement, asynchronous mesh baking, and tier handoff.
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use bevy::{
    camera::primitives::{Aabb, MeshAabb},
    light::NotShadowCaster,
    math::DVec3,
    prelude::*,
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
};
use world_core::{
    planet::RADIUS,
    runtime::{GridTransform, PlanetRoot, ReferenceFrame, ReferenceFrames, WorldCamera},
    SurfaceHeights,
};

use super::{
    hero_oak::HeroOakAssets,
    impostor,
    lod::{coverage, fade_step, quantize, spawn_tree, VegetationSettings, View},
    placement::{NodeKey, PatchKey, Site, PATCH_METERS},
    proxy::{ProxyMaterial, ProxyMesh, ProxyUniform, CHILD_SLOTS},
    within_horizon, ForestClearing, ForestModels,
};

const MAX_NODES: usize = 112;
const NODE_ALLOWANCE: f32 = 480.0;
const DISCOVERY_INTERVAL: f64 = 0.16;
const PLACEMENT_JOBS: usize = 12;
const PARENT_JOBS: usize = 3;
const LEAF_JOBS: usize = 5;
// Give close-range tree meshes enough room in tropical stands; otherwise
// whole patches fall back to billboard silhouettes even next to the player.
const MAX_DETAILS: usize = 600;
const SPAWNS_PER_FRAME: usize = 40;
const RETIREMENTS_PER_FRAME: usize = 34;
const MAX_BATCH_UPLOADS: usize = 4;
const MAX_BATCH_RETIREMENTS: usize = 12;

type Sites = Arc<[Site]>;

struct Leaf {
    key: PatchKey,
    anchor: DVec3,
    sites: Option<Sites>,
    placement: Option<Task<Sites>>,
    impostor: MeshState,
    trees: Vec<Entity>,
    coverage: f32,
    detail: f32,
}

struct Node {
    anchor: DVec3,
    leaves: Vec<Leaf>,
    parent: MeshState,
    arrival: f32,
}

struct Batch {
    entity: Entity,
    material: Handle<ProxyMaterial>,
    bounds: Aabb,
}

#[derive(Default)]
enum MeshState {
    #[default]
    Missing,
    Pending(Task<Option<Mesh>>),
    Empty,
    Ready(Batch),
}

impl MeshState {
    fn ready(&self) -> Option<&Batch> {
        match self {
            Self::Ready(batch) => Some(batch),
            _ => None,
        }
    }

    fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    fn retire(&mut self, commands: &mut Commands, retiring: &mut Vec<Entity>) {
        // Dropping a pending Bevy task cancels undispatched work.
        if let Self::Ready(batch) = std::mem::take(self) {
            commands.entity(batch.entity).insert(Visibility::Hidden);
            retiring.push(batch.entity);
        }
    }

    fn poll(&mut self) -> Option<Option<Mesh>> {
        match self {
            Self::Pending(task) => block_on(poll_once(task)),
            _ => None,
        }
    }
}

#[derive(Resource, Default)]
pub(super) struct ForestStream {
    nodes: HashMap<NodeKey, Node>,
    order: Vec<NodeKey>,
    mask: HashMap<PatchKey, (f32, f32)>,
    retiring: Vec<Entity>,
    retiring_batches: Vec<Entity>,
    atlas: Option<Handle<Image>>,
    atlas_task: Option<Task<Image>>,
    atlas_revision: (bool, usize),
    atlas_task_revision: (bool, usize),
    next_debug_log: f64,
    next_plan: f64,
    last_eye: Option<DVec3>,
    last_view: DVec3,
    settings_extent: f32,
    pub stats: StreamStats,
}

#[derive(Default)]
pub(super) struct StreamStats {
    pub nodes: usize,
    pub retirement_queue: usize,
    pub unsampled: usize,
    pub parents: usize,
    pub impostors: usize,
    pub jobs: usize,
    pub trees: usize,
}

impl ForestStream {
    pub fn sites(&self, key: PatchKey) -> Option<&[Site]> {
        self.nodes
            .get(&key.parent())?
            .leaves
            .iter()
            .find(|leaf| leaf.key == key)?
            .sites
            .as_deref()
    }

    pub fn coverage(&self, patch: PatchKey) -> (f32, f32) {
        self.mask.get(&patch).copied().unwrap_or_default()
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
            node.parent.retire(commands, &mut self.retiring_batches);
            for leaf in &mut node.leaves {
                leaf.impostor.retire(commands, &mut self.retiring_batches);
                self.retiring.append(&mut leaf.trees);
            }
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
                        impostor: MeshState::Missing,
                        trees: Vec::new(),
                        coverage: 0.0,
                        detail: 0.0,
                    })
                    .collect();
                Node {
                    anchor: location,
                    leaves,
                    parent: MeshState::Missing,
                    arrival: 0.0,
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
                    let mut sites = key.sites(&heights);
                    if let Some(clearing) = clearing {
                        sites.retain(|site| !clearing.contains(site.position));
                    }
                    sites.into()
                }));
                budget -= 1;
            }
        }
    }

    fn request_meshes(&mut self, eye: DVec3, impostor_range: f32, preload: f32) {
        let pool = AsyncComputeTaskPool::get();
        let mut parents = PARENT_JOBS.saturating_sub(
            self.nodes
                .values()
                .filter(|node| matches!(node.parent, MeshState::Pending(_)))
                .count(),
        );
        let mut leaves = LEAF_JOBS.saturating_sub(
            self.nodes
                .values()
                .flat_map(|node| &node.leaves)
                .filter(|leaf| matches!(leaf.impostor, MeshState::Pending(_)))
                .count(),
        );
        for key in &self.order {
            let node = self.nodes.get_mut(key).unwrap();
            if parents > 0
                && node.parent.is_missing()
                && node.leaves.iter().all(|leaf| leaf.sites.is_some())
            {
                let children: Vec<_> = node
                    .leaves
                    .iter()
                    .map(|leaf| leaf.sites.as_ref().unwrap().clone())
                    .collect();
                debug_assert!(children.len() <= CHILD_SLOTS);
                let origin = node.anchor;
                node.parent = MeshState::Pending(pool.spawn(async move {
                    let mut mesh = ProxyMesh::default();
                    for (slot, sites) in children.iter().enumerate() {
                        for &site in sites.iter() {
                            mesh.add(site, origin, slot);
                        }
                    }
                    mesh.finish()
                }));
                parents -= 1;
            }
            if leaves == 0 || self.atlas.is_none() {
                continue;
            }
            let reach = f64::from(impostor_range + preload.min(180.0));
            for leaf in &mut node.leaves {
                let Some(sites) = &leaf.sites else {
                    continue;
                };
                if leaves == 0 || !leaf.impostor.is_missing() || leaf.anchor.distance(eye) > reach {
                    continue;
                }
                if sites.is_empty() {
                    leaf.impostor = MeshState::Empty;
                    continue;
                }
                let sites = sites.clone();
                let origin = leaf.anchor;
                leaf.impostor = MeshState::Pending(pool.spawn(async move {
                    let mut mesh = ProxyMesh::default();
                    for &site in sites.iter() {
                        mesh.add(site, origin, 0);
                    }
                    mesh.finish()
                }));
                leaves -= 1;
            }
        }
    }

    fn detail_requests(
        &mut self,
        commands: &mut Commands,
        root: &PlanetRoot,
        frame: &ReferenceFrame,
        eye: DVec3,
        band: Vec2,
        step: f32,
    ) {
        let mut requests: Vec<_> = self
            .nodes
            .iter_mut()
            .flat_map(|(&key, node)| {
                node.leaves
                    .iter_mut()
                    .filter(|leaf| leaf.impostor.ready().is_some())
                    .map(move |leaf| (eye.distance(leaf.anchor), key, leaf))
            })
            .collect();
        requests.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.2.key.cmp(&b.2.key)));
        let mut admitted = 0;
        let mut resident = requests
            .iter()
            .map(|(_, _, leaf)| leaf.trees.len())
            .sum::<usize>()
            + self.retiring.len();
        let mut spawn_budget = SPAWNS_PER_FRAME;
        for (distance, _, leaf) in requests {
            let sites = leaf.sites.as_ref().unwrap();
            let margin = PATCH_METERS * if leaf.trees.is_empty() { 0.79 } else { 0.95 };
            let wanted =
                distance <= f64::from(band.y) + margin && admitted + sites.len() <= MAX_DETAILS;
            if wanted {
                admitted += sites.len();
                let begin = leaf.trees.len();
                let count = sites
                    .len()
                    .saturating_sub(begin)
                    .min(spawn_budget)
                    .min(MAX_DETAILS.saturating_sub(resident));
                for &site in &sites[begin..begin + count] {
                    leaf.trees
                        .push(spawn_tree(commands, root, frame, leaf.key, site));
                }
                spawn_budget -= count;
                resident += count;
            }
            let target = if wanted && leaf.trees.len() == sites.len() {
                1.0
            } else {
                0.0
            };
            leaf.detail = approach(leaf.detail, target, step);
            if !wanted && leaf.detail == 0.0 {
                self.retiring.append(&mut leaf.trees);
            }
        }
    }
}

fn anchor(heights: &SurfaceHeights, direction: DVec3) -> DVec3 {
    direction * (RADIUS + heights.sample(direction).unwrap_or(0.0))
}

fn approach(from: f32, to: f32, step: f32) -> f32 {
    from + (to - from).clamp(-step, step)
}

fn proxy_parameters(forest: f32, range: f32, band: Vec2, is_parent: bool) -> ProxyUniform {
    ProxyUniform {
        leaf: Vec4::new(0.0, 0.0, band.x, band.y),
        forest: Vec4::new(forest * 0.79, forest, 0.0, f32::from(u8::from(is_parent))),
        impostor: Vec4::new(range * 0.78, range, 0.0, 0.0),
        children: [Vec4::ZERO; CHILD_SLOTS / 4],
    }
}

fn make_batch(
    commands: &mut Commands,
    root: &PlanetRoot,
    frame: &ReferenceFrame,
    mesh: Mesh,
    anchor: DVec3,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<ProxyMaterial>,
    material: ProxyMaterial,
) -> Option<Batch> {
    let mut bounds = mesh.get_aabb()?;
    // Billboards rotate on the GPU. Keep their full swept volume inside the CPU bounds.
    bounds.half_extents += 27.0;
    let (cell, translation) = frame.translation_to_grid(anchor);
    let material = materials.add(material);
    let entity = root
        .spawn(
            commands,
            (
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material.clone()),
                Transform::from_translation(translation),
                Visibility::Hidden,
                bounds,
                NotShadowCaster,
            ),
        )
        .insert(cell)
        .id();
    Some(Batch {
        entity,
        material,
        bounds,
    })
}

struct RenderAssets<'a> {
    meshes: &'a mut Assets<Mesh>,
    materials: &'a mut Assets<ProxyMaterial>,
}

fn upload(
    commands: &mut Commands,
    root: &PlanetRoot,
    frame: &ReferenceFrame,
    state: &mut MeshState,
    anchor: DVec3,
    assets: &mut RenderAssets,
    material: impl FnOnce() -> ProxyMaterial,
) -> bool {
    let Some(ready) = state.poll() else {
        return false;
    };
    *state = ready
        .and_then(|mesh| {
            make_batch(
                commands,
                root,
                frame,
                mesh,
                anchor,
                assets.meshes,
                assets.materials,
                material(),
            )
        })
        .map_or(MeshState::Empty, MeshState::Ready);
    matches!(state, MeshState::Ready(_))
}

fn show_batch(
    commands: &mut Commands,
    visibility: &mut Query<&mut Visibility, With<MeshMaterial3d<ProxyMaterial>>>,
    batch: &Batch,
    visible: bool,
) {
    let desired = if visible {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    if let Ok(mut current) = visibility.get_mut(batch.entity) {
        current.set_if_neq(desired);
    } else {
        // A freshly uploaded batch is still in the command queue.
        commands.entity(batch.entity).insert(desired);
    }
}

pub(super) fn stream(
    mut commands: Commands,
    settings: Res<VegetationSettings>,
    time: Res<Time>,
    root: Res<PlanetRoot>,
    heights: Res<SurfaceHeights>,
    clearing: Option<Res<ForestClearing>>,
    cameras: Query<(Entity, GridTransform, &Camera, &Projection), With<WorldCamera>>,
    frames: ReferenceFrames,
    mut forest: ResMut<ForestStream>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<ProxyMaterial>>,
    models: Res<ForestModels>,
    hero_loading: Res<HeroOakAssets>,
    mut visibility: Query<&mut Visibility, With<MeshMaterial3d<ProxyMaterial>>>,
) {
    let Ok((entity, camera, view, projection)) = cameras.single() else {
        return;
    };
    let Some(frame) = frames.parent_frame(entity) else {
        return;
    };
    let eye = frame.grid_position_double(&camera.cell, &camera.transform);
    let Some(up) = eye.try_normalize() else {
        return;
    };
    let forward = (camera.transform.rotation * Vec3::NEG_Z).as_dvec3();
    let band = View::new(view, projection).impostor_band(&settings);
    let range = settings.impostor_range.clamp(200.0, 600.0);
    let forest_range = settings.forest_range.clamp(range + 220.0, 3_000.0);
    let preload = settings.preload_margin.clamp(90.0, 420.0);
    let step = fade_step(&time, &settings);
    // Keep the atlas handle and UV layout stable while new portraits arrive.
    let oak = models
        .hero_oak
        .as_ref()
        .and_then(|hero| images.get(&hero.impostor_image));
    let revision = (oak.is_some(), models.portraits.revision);
    if forest.atlas_task.is_none() && (forest.atlas.is_none() || forest.atlas_revision != revision)
    {
        let oak = oak.cloned();
        let portraits = models.portraits.clone();
        forest.atlas_task_revision = revision;
        forest.atlas_task = Some(
            AsyncComputeTaskPool::get()
                .spawn(async move { impostor::bake(oak.as_ref(), &portraits) }),
        );
    }
    if let Some(image) = forest
        .atlas_task
        .as_mut()
        .and_then(|task| block_on(poll_once(task)))
    {
        if let Some(atlas) = forest.atlas.as_ref() {
            // Check existence before taking AssetMut: the guard from get_mut()
            // otherwise keeps the mutable borrow alive across the else branch.
            if images.get(atlas).is_some() {
                *images.get_mut(atlas).expect("atlas was checked above") = image;
            } else {
                forest.atlas = Some(images.add(image));
            }
        } else {
            forest.atlas = Some(images.add(image));
        }
        forest.atlas_revision = forest.atlas_task_revision;
        forest.atlas_task = None;
    }

    forest.plan(
        &mut commands,
        eye,
        up,
        forward,
        time.elapsed_secs_f64(),
        forest_range + preload,
        &heights,
    );
    forest.allocate_sites(&heights, eye, clearing.as_deref());
    forest.request_meshes(eye, range, preload);
    let retired = forest.retiring.len().saturating_sub(RETIREMENTS_PER_FRAME);
    for tree in forest.retiring.drain(retired..) {
        commands.entity(tree).despawn();
    }

    let retired_batches = forest
        .retiring_batches
        .len()
        .saturating_sub(MAX_BATCH_RETIREMENTS);
    for batch in forest.retiring_batches.drain(retired_batches..) {
        commands.entity(batch).despawn();
    }
    let forest = &mut *forest;
    forest.stats = StreamStats {
        nodes: forest.nodes.len(),
        retirement_queue: forest.retiring.len() + forest.retiring_batches.len(),
        ..default()
    };
    let mut assets = RenderAssets {
        meshes: &mut meshes,
        materials: &mut materials,
    };
    if let Some(atlas) = forest.atlas.as_ref() {
        let mut uploads = MAX_BATCH_UPLOADS;
        for key in &forest.order {
            let node = forest.nodes.get_mut(key).unwrap();
            for leaf in &mut node.leaves {
                if uploads > 1
                    && upload(
                        &mut commands,
                        &root,
                        frame,
                        &mut leaf.impostor,
                        leaf.anchor,
                        &mut assets,
                        || ProxyMaterial {
                            coverage: proxy_parameters(forest_range, range, band, false),
                            atlas: atlas.clone(),
                        },
                    )
                {
                    uploads -= 1;
                }
            }
        }
        for key in &forest.order {
            if uploads == 0 {
                break;
            }
            let node = forest.nodes.get_mut(key).unwrap();
            if upload(
                &mut commands,
                &root,
                frame,
                &mut node.parent,
                node.anchor,
                &mut assets,
                || ProxyMaterial {
                    coverage: proxy_parameters(forest_range, range, band, true),
                    atlas: atlas.clone(),
                },
            ) {
                uploads -= 1;
            }
        }
    }
    forest.detail_requests(&mut commands, &root, frame, eye, band, step);

    forest.mask.clear();
    for key in &forest.order {
        let node = forest.nodes.get_mut(key).unwrap();
        let mut uniform = proxy_parameters(forest_range, range, band, true);
        for (slot, leaf) in node.leaves.iter_mut().enumerate() {
            let distance = eye.distance(leaf.anchor) as f32;
            let child_target = f32::from(u8::from(
                leaf.impostor.ready().is_some() && distance < range + PATCH_METERS as f32,
            ));
            leaf.coverage = approach(leaf.coverage, child_target, step);
            let child = coverage(quantize(leaf.coverage));
            let detailed = coverage(quantize(leaf.detail));
            uniform.children[slot / 4][slot % 4] = child;
            if child != 0.0 && detailed != 0.0 {
                forest.mask.insert(leaf.key, (child, detailed));
            }
            if child == 0.0
                && leaf.trees.is_empty()
                && distance > range + preload.min(180.0) + 120.0
                && !matches!(leaf.impostor, MeshState::Empty)
            {
                leaf.impostor
                    .retire(&mut commands, &mut forest.retiring_batches);
            }
            if let Some(batch) = leaf.impostor.ready() {
                if child != 0.0 {
                    let new = Vec4::new(child, detailed, band.x, band.y);
                    if assets
                        .materials
                        .get(batch.material.id())
                        .is_some_and(|mat| {
                            mat.coverage.leaf != new || mat.coverage.impostor != uniform.impostor
                        })
                    {
                        if let Some(mut material) = assets.materials.get_mut(batch.material.id()) {
                            material.coverage.leaf = new;
                            material.coverage.impostor = uniform.impostor;
                        }
                    }
                }
                show_batch(&mut commands, &mut visibility, batch, child != 0.0);
                forest.stats.impostors += 1;
            }
            forest.stats.unsampled += usize::from(leaf.sites.is_none());
            forest.stats.jobs += usize::from(leaf.placement.is_some())
                + usize::from(matches!(leaf.impostor, MeshState::Pending(_)));
            forest.stats.trees += leaf.trees.len();
        }
        forest.stats.jobs += usize::from(matches!(node.parent, MeshState::Pending(_)));
        if let Some(batch) = node.parent.ready() {
            node.arrival = (node.arrival + step).min(1.0);
            uniform.forest.z = coverage(quantize(node.arrival));
            let center = node.anchor + Vec3::from(batch.bounds.center).as_dvec3();
            let radius = f64::from(batch.bounds.half_extents.length());
            let in_range = eye.distance(center) - radius < f64::from(forest_range)
                && within_horizon(eye, center, radius);
            show_batch(&mut commands, &mut visibility, batch, in_range);
            if in_range {
                let changed = assets
                    .materials
                    .get(batch.material.id())
                    .is_some_and(|mat| {
                        mat.coverage.forest != uniform.forest
                            || mat.coverage.children != uniform.children
                            || mat.coverage.impostor != uniform.impostor
                    });
                if changed {
                    if let Some(mut material) = assets.materials.get_mut(batch.material.id()) {
                        material.coverage = uniform;
                    }
                }
            }
            forest.stats.parents += 1;
        }
    }
    // Diagnose all stages of tree streaming, not only the empty-batch case:
    // a resident batch can still be invisible due to its material or culling.
    let now = time.elapsed_secs_f64();
    if now >= forest.next_debug_log {
        forest.next_debug_log = now + 5.0;
        let (sampled, placements) = forest.nodes.values().flat_map(|node| &node.leaves).fold(
            (0_usize, 0_usize),
            |(sampled, placements), leaf| match leaf.sites.as_ref() {
                Some(sites) => (sampled + 1, placements + sites.len()),
                None => (sampled, placements),
            },
        );
        let altitude = eye.length() - RADIUS - heights.sample(up).unwrap_or(0.0);
        info!(
            "Forest stream: nodes={} sampled_patches={} tree_sites={} parents={} impostors={} detail_entities={} jobs={} atlas={} atlas_baking={} hero_pending={} altitude={:.1}m",
            forest.stats.nodes,
            sampled,
            placements,
            forest.stats.parents,
            forest.stats.impostors,
            forest.stats.trees,
            forest.stats.jobs,
            forest.atlas.is_some(),
            forest.atlas_task.is_some(),
            hero_loading.is_pending(),
            altitude,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource)]
    struct Fixture {
        state: MeshState,
        retire: Vec<Entity>,
    }

    #[test]
    fn retiring_a_tile_unlinks_it_from_the_floating_origin_hierarchy() {
        let mut app = App::new();
        let parent = app.world_mut().spawn_empty().id();
        let child = app
            .world_mut()
            .spawn((Visibility::Inherited, ChildOf(parent)))
            .id();
        app.insert_resource(Fixture {
            state: MeshState::Ready(Batch {
                entity: child,
                material: Handle::default(),
                bounds: Aabb::default(),
            }),
            retire: Vec::new(),
        })
        .add_systems(
            Update,
            |mut commands: Commands, mut fixture: ResMut<Fixture>| {
                let Fixture { state, retire } = &mut *fixture;
                state.retire(&mut commands, retire);
                for entity in retire.drain(..) {
                    commands.entity(entity).despawn();
                }
            },
        );

        app.update();
        assert!(app.world().get_entity(child).is_err());
        assert!(app
            .world()
            .get::<Children>(parent)
            .is_none_or(|children| !children.contains(&child)));
    }
}

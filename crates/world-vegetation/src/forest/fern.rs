//! Woodland Fern streaming and dithered LOD handoff.
mod assets;
mod placement;

use std::collections::{HashMap, HashSet};

use bevy::{
    math::DVec3,
    pbr::{
        ExtendedMaterial, MaterialExtension, MaterialPlugin, NotShadowCaster, NotShadowReceiver,
    },
    prelude::*,
    reflect::TypePath,
    render::{
        primitives::Aabb,
        render_asset::RenderAssetUsages,
        render_resource::{AsBindGroup, ShaderRef},
    },
};
use world_core::{
    planet::RADIUS,
    runtime::{GridTransform, PlanetRoot, ReferenceFrame, ReferenceFrames, WorldCamera},
    SurfaceHeights,
};

use self::{
    assets::{FernAssets, Models},
    placement::{nearby_trunks, Patch, Plant, MAX_PLANTS, PATCH_ALLOWANCE},
};
use super::{
    lod::{coverage, dither_material, quantize, smoothstep, BarkMaterial, LodState},
    placement::PatchKey,
    streaming::{self, ForestStream},
    MeshBuilder,
};

const FAR: usize = 3;
const MAX_PATCHES: usize = 168;
const DISCOVERY_INTERVAL: f64 = 0.14;
const CHUNKS_PER_FRAME: usize = 5;
const DETAIL_SPAWNS_PER_FRAME: usize = 24;
const RETIREMENTS_PER_FRAME: usize = 24;

/// Woodland Fern LOD distances and streaming settings, in world meters.
#[derive(Clone, Copy, Resource)]
pub struct FernSettings {
    /// Upper distances for LOD0, LOD1, LOD2, and the LOD3 outer fade.
    pub distances: [f32; 4],
    /// Fractional hysteresis at the three mesh boundaries.
    pub hysteresis: f32,
    /// Distance beyond the last band to load chunks before they become visible.
    pub preload_margin: f32,
    /// Duration of a full dither transition in seconds.
    pub transition_seconds: f32,
    /// Maximum number of resident LOD0 fern meshes.
    pub max_lod0: usize,
}

impl Default for FernSettings {
    fn default() -> Self {
        Self {
            distances: [15.0, 35.0, 70.0, 120.0],
            hysteresis: 0.10,
            preload_margin: 30.0,
            transition_seconds: 0.30,
            max_lod0: 12,
        }
    }
}

impl FernSettings {
    fn distances(&self) -> [f32; 4] {
        let defaults = Self::default().distances;
        let [a, b, c, d] = std::array::from_fn(|i| {
            if self.distances[i].is_finite() {
                self.distances[i]
            } else {
                defaults[i]
            }
        });
        let a = a.clamp(1.0, 178.0);
        let b = b.clamp(a + 4.0, 182.0);
        let c = c.clamp(b + 4.0, 186.0);
        [a, b, c, d.clamp(c + 6.0, 195.0)]
    }
}

fn choose_lod(distance: f32, bands: [f32; 4], h: f32, last: Option<usize>) -> usize {
    let mut lod = last.unwrap_or_else(|| {
        bands[..FAR]
            .iter()
            .position(|&end| distance < end)
            .unwrap_or(FAR)
    });
    while lod < FAR && distance > bands[lod] * (1.0 + h) {
        lod += 1;
    }
    while lod > 0 && distance < bands[lod - 1] * (1.0 - h) {
        lod -= 1;
    }
    lod
}

pub(super) struct FernPlugin;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub(super) struct FernStreaming;

impl Plugin for FernPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FernSettings>()
            .init_resource::<FernWorld>()
            .add_plugins(MaterialPlugin::<BatchMaterial> {
                prepass_enabled: false,
                shadows_enabled: false,
                ..default()
            })
            .add_systems(Startup, assets::request)
            .add_systems(
                Update,
                (assets::finish, stream)
                    .chain()
                    .after(streaming::stream)
                    .in_set(FernStreaming),
            );
    }
}

// Each vec4 contains the complementary dither intervals of two ferns.
type MaskArray = [Vec4; MAX_PLANTS / 2];
type Mask = (u8, u8);

#[derive(Clone, Asset, AsBindGroup, TypePath)]
struct BatchFade {
    #[uniform(100)]
    masks: MaskArray,
}

impl MaterialExtension for BatchFade {
    fn fragment_shader() -> ShaderRef {
        "shaders/woodland_fern_batch.wgsl".into()
    }
}
type BatchMaterial = ExtendedMaterial<StandardMaterial, BatchFade>;

struct Slot {
    entity: Entity,
    mask: Mask,
}

struct Fern {
    plant: Plant,
    lod: LodState<usize>,
    details: [Option<Slot>; FAR],
}

impl Fern {
    fn new(plant: Plant) -> Self {
        let mut lod = LodState::default();
        lod.current = Some(FAR);
        Self {
            plant,
            lod,
            details: std::array::from_fn(|_| None),
        }
    }

    fn masks(state: &LodState<usize>, upper: u8) -> [Option<Mask>; 4] {
        let mut result = [None; 4];
        for (level, low, high) in state
            .slots(upper)
            .expect("fern is initialized with a resident impostor")
        {
            if high > low {
                result[level] = Some((low, high));
            }
        }
        result
    }
}

struct Chunk {
    anchor: DVec3,
    plants: Vec<Fern>,
    entity: Entity,
    material: Handle<BatchMaterial>,
    visible: bool,
    arrival: f32,
}

#[derive(Default)]
pub(super) struct Metrics {
    pub levels: [usize; 4],
    pub blends: usize,
    pub chunks: usize,
    pub batches: usize,
    pub waiting: usize,
    pub status: &'static str,
}

#[derive(Resource, Default)]
pub(super) struct FernWorld {
    chunks: HashMap<Patch, Option<Chunk>>,
    order: Vec<Patch>,
    retiring: Vec<Entity>,
    materials: HashMap<Mask, Handle<BarkMaterial>>,
    next_plan: f64,
    last_eye: Option<DVec3>,
    extent: f64,
    pub metrics: Metrics,
}

impl FernWorld {
    fn plan(
        &mut self,
        commands: &mut Commands,
        eye: DVec3,
        reach: f64,
        heights: &SurfaceHeights,
        time: f64,
    ) {
        if self.extent == reach
            && self.last_eye.is_some_and(|last| {
                let moved = last.distance_squared(eye);
                moved < 64.0 || (time < self.next_plan && moved < 625.0)
            })
        {
            return;
        }
        self.last_eye = Some(eye);
        self.next_plan = time + DISCOVERY_INTERVAL;
        self.extent = reach;
        let up = eye.normalize_or_zero();
        let ground = heights.sample(up).unwrap_or(0.0);
        let altitude = eye.length() - RADIUS - ground;
        self.order = if altitude <= reach + PATCH_ALLOWANCE {
            Patch::around(up, reach)
                .into_iter()
                .take(MAX_PATCHES)
                .collect()
        } else {
            Vec::new()
        };
        // Leave a second ring around requested tiles. A retiring patch is already
        // outside the visible range, including its furthest fern.
        let wanted: HashSet<_> = self.order.iter().copied().collect();
        self.chunks.retain(|key, value| {
            if wanted.contains(key)
                || (altitude <= reach + PATCH_ALLOWANCE
                    && key.center().distance(up) * RADIUS < reach + 18.0)
            {
                return true;
            }
            if let Some(chunk) = value {
                commands.entity(chunk.entity).insert(Visibility::Hidden);
                self.retiring.push(chunk.entity);
                for fern in &mut chunk.plants {
                    for slot in fern.details.iter_mut().filter_map(Option::take) {
                        commands.entity(slot.entity).insert(Visibility::Hidden);
                        self.retiring.push(slot.entity);
                    }
                }
            }
            false
        });
    }

    fn prepare(
        &mut self,
        commands: &mut Commands,
        root: &PlanetRoot,
        frame: &ReferenceFrame,
        models: &Models,
        heights: &SurfaceHeights,
        forest: &ForestStream,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<BatchMaterial>,
    ) -> usize {
        let mut budget = CHUNKS_PER_FRAME;
        let mut waiting = 0;
        for &key in &self.order {
            if self.chunks.contains_key(&key) {
                continue;
            }
            waiting += 1;
            if budget == 0 || self.chunks.len() >= MAX_PATCHES + 32 {
                continue;
            }
            let up = key.center();
            let Some(height) = heights.sample(up) else {
                self.chunks.insert(key, None);
                waiting -= 1;
                budget -= 1;
                continue;
            };
            let origin = up * (RADIUS + height);
            // Wait for canonical placements so discovery order cannot change shade.
            let Some(trunks) = PatchKey::shade_neighborhood(up)
                .map(|tree_patch| forest.sites(tree_patch))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            let trees = nearby_trunks(
                origin,
                trunks.into_iter().flatten().map(|tree| tree.position),
            );
            let plants = key.grow(heights, &trees);
            self.chunks.insert(
                key,
                make_chunk(
                    commands, root, frame, models, origin, plants, meshes, materials,
                ),
            );
            budget -= 1;
            waiting -= 1;
        }
        waiting
    }
}

fn make_chunk(
    commands: &mut Commands,
    root: &PlanetRoot,
    frame: &ReferenceFrame,
    models: &Models,
    origin: DVec3,
    plants: Vec<Plant>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<BatchMaterial>,
) -> Option<Chunk> {
    if plants.is_empty() {
        return None;
    }
    let mesh = batch_mesh(&models.cards, origin, &plants);
    let bounds = mesh.compute_aabb()?;
    let (cell, translation) = frame.translation_to_grid(origin);
    let material = materials.add(BatchMaterial {
        base: models.card_material.clone(),
        extension: BatchFade {
            masks: [Vec4::ZERO; MAX_PLANTS / 2],
        },
    });
    let entity = root
        .spawn(
            commands,
            (
                cell,
                MaterialMeshBundle {
                    mesh: meshes.add(mesh),
                    material: material.clone(),
                    transform: Transform::from_translation(translation),
                    visibility: Visibility::Hidden,
                    ..default()
                },
                bounds,
                NotShadowCaster,
                NotShadowReceiver,
            ),
        )
        .id();
    Some(Chunk {
        anchor: origin,
        plants: plants.into_iter().map(Fern::new).collect(),
        entity,
        material,
        visible: false,
        arrival: 0.0,
    })
}

fn batch_mesh(cards: &MeshBuilder, origin: DVec3, plants: &[Plant]) -> Mesh {
    let mut merged = MeshBuilder::default();
    for (id, plant) in plants.iter().enumerate() {
        let base = merged.positions.len() as u32;
        let local = (plant.position - origin).as_vec3();
        for i in 0..cards.positions.len() {
            let position =
                local + plant.rotation * (Vec3::from_array(cards.positions[i]) * plant.scale);
            let normal = plant.rotation * Vec3::from_array(cards.normals[i]);
            merged.vertex(
                position,
                normal,
                Vec2::from_array(cards.uvs[i]),
                cards.colors[i],
            );
            merged.wind.push([id as f32, 0.0]);
        }
        merged
            .indices
            .extend(cards.indices.iter().map(|index| index + base));
    }
    let mut mesh = merged.finish();
    mesh.asset_usage = RenderAssetUsages::RENDER_WORLD;
    mesh
}

fn set_batch_mask(masks: &mut MaskArray, plant: usize, mask: Mask) {
    let slots = &mut masks[plant / 2];
    let i = plant % 2 * 2;
    slots[i] = coverage(mask.0);
    slots[i + 1] = coverage(mask.1);
}

fn stream(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<FernSettings>,
    heights: Res<SurfaceHeights>,
    forest: Res<ForestStream>,
    models: Res<FernAssets>,
    root: Res<PlanetRoot>,
    frames: ReferenceFrames,
    cameras: Query<(Entity, GridTransform, &Camera), With<WorldCamera>>,
    mut world: ResMut<FernWorld>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut far_materials: ResMut<Assets<BatchMaterial>>,
    mut materials: ResMut<Assets<BarkMaterial>>,
) {
    let world = &mut *world;
    world.metrics = Metrics {
        status: models.label(),
        ..default()
    };
    let FernAssets::Ready(models) = &*models else {
        return;
    };
    let Ok((id, grid, camera)) = cameras.single() else {
        return;
    };
    if !camera.is_active {
        return;
    }
    let Some(frame) = frames.parent_frame(id) else {
        return;
    };
    let eye = frame.grid_position_double(&grid.cell, &grid.transform);
    let bands = settings.distances();
    let outer = bands[FAR];
    let reach = f64::from(outer + settings.preload_margin.clamp(10.0, 65.0)) + PATCH_ALLOWANCE;
    world.plan(
        &mut commands,
        eye,
        reach,
        &heights,
        time.elapsed_secs_f64(),
    );
    world.metrics.waiting = world.prepare(
        &mut commands,
        &root,
        frame,
        models,
        &heights,
        &forest,
        &mut meshes,
        &mut far_materials,
    );
    let start = world.retiring.len().saturating_sub(RETIREMENTS_PER_FRAME);
    for entity in world.retiring.drain(start..) {
        commands.entity(entity).despawn_recursive();
    }

    let h = settings.hysteresis.clamp(0.0, 0.25);
    let step = time.delta_secs().min(0.075) / settings.transition_seconds.clamp(0.10, 2.0);
    let mut nearest = Vec::new();
    let mut lod0_count = 0;
    for (&key, chunk) in &world.chunks {
        let Some(chunk) = chunk else {
            continue;
        };
        for (i, fern) in chunk.plants.iter().enumerate() {
            let distance = eye.distance(fern.plant.position) as f32;
            let resident = fern.details[0].is_some();
            lod0_count += usize::from(resident);
            if choose_lod(distance, bands, h, fern.lod.desired) == 0 {
                nearest.push((distance - if resident { 0.8 } else { 0.0 }, key, i));
            }
        }
    }
    nearest.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let limit = settings.max_lod0.min(64);
    let lod0: HashSet<_> = nearest
        .into_iter()
        .take(limit)
        .map(|(_, key, i)| (key, i))
        .collect();
    let mut high_slots = limit.saturating_sub(lod0_count);
    let mut spawns = DETAIL_SPAWNS_PER_FRAME;

    // Sorting only occupied tiles keeps upload and LOD admission deterministic
    // even when hash-map iteration or travel direction changes.
    let mut occupied: Vec<_> = world
        .chunks
        .iter_mut()
        .filter_map(|(&key, chunk)| {
            let chunk = chunk.as_mut()?;
            Some((chunk.anchor.distance_squared(eye), key, chunk))
        })
        .collect();
    occupied.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    for (_, key, chunk) in occupied {
        world.metrics.chunks += 1;
        chunk.arrival = (chunk.arrival + step).min(1.0);
        let mut batch = [Vec4::ZERO; MAX_PLANTS / 2];
        let mut show = false;
        for (i, fern) in chunk.plants.iter_mut().enumerate() {
            let distance = eye.distance(fern.plant.position) as f32;
            let outer_fade = 1.0 - smoothstep(outer - (outer * 0.13).min(14.0), outer, distance);
            let upper = quantize(chunk.arrival * outer_fade);
            let mut goal = choose_lod(distance, bands, h, fern.lod.desired);
            if goal == 0
                && (!lod0.contains(&(key, i)) || (fern.details[0].is_none() && high_slots == 0))
            {
                goal = 1;
            }
            let mut next = fern.lod;
            if upper == 0 {
                next = Fern::new(fern.plant).lod;
            } else {
                next.update(goal, step);
            }
            let mut masks = Fern::masks(&next, upper);
            let needed = (0..FAR)
                .filter(|&level| masks[level].is_some() && fern.details[level].is_none())
                .count();
            if needed <= spawns
                && (masks[0].is_none() || fern.details[0].is_some() || high_slots > 0)
            {
                fern.lod = next;
            } else {
                masks = resident_masks(Fern::masks(&fern.lod, upper), &fern.details, upper);
            }
            world.metrics.blends += usize::from(upper != 0 && fern.lod.is_blending());
            for (level, mask) in masks[..FAR].iter().copied().enumerate() {
                if mask.is_some() {
                    world.metrics.levels[level] += 1;
                }
                let detail = &mut fern.details[level];
                match (detail.as_mut(), mask) {
                    (Some(slot), Some(mask)) => {
                        if slot.mask != mask {
                            slot.mask = mask;
                            let material =
                                shared_material(&mut world.materials, &mut materials, models, mask);
                            commands.entity(slot.entity).insert(material);
                        }
                    }
                    (None, Some(mask)) if spawns > 0 => {
                        spawns -= 1;
                        if level == 0 {
                            high_slots = high_slots.saturating_sub(1);
                        }
                        let material =
                            shared_material(&mut world.materials, &mut materials, models, mask);
                        let entity = spawn_detail(
                            &mut commands,
                            &root,
                            frame,
                            fern.plant,
                            &models.detail[level],
                            material,
                        );
                        *detail = Some(Slot { entity, mask });
                    }
                    (Some(slot), None) => {
                        commands.entity(slot.entity).insert(Visibility::Hidden);
                        world.retiring.push(slot.entity);
                        *detail = None;
                    }
                    _ => {}
                }
            }
            if let Some(mask) = masks[FAR] {
                world.metrics.levels[FAR] += 1;
                set_batch_mask(&mut batch, i, mask);
                show = true;
            }
        }
        if far_materials
            .get(&chunk.material)
            .is_some_and(|material| material.extension.masks != batch)
        {
            far_materials
                .get_mut(&chunk.material)
                .unwrap()
                .extension
                .masks = batch;
        }
        if chunk.visible != show {
            commands.entity(chunk.entity).insert(if show {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
            chunk.visible = show;
        }
        world.metrics.batches += usize::from(show);
    }
}

fn resident_masks(
    mut masks: [Option<Mask>; 4],
    details: &[Option<Slot>; FAR],
    upper: u8,
) -> [Option<Mask>; 4] {
    // A rounded interval can become nonzero as coverage increases while the
    // upload budget is exhausted. Give those pixels to the resident side.
    let mut deferred = false;
    for (mask, detail) in masks[..FAR].iter_mut().zip(details) {
        if detail.is_none() && mask.take().is_some() {
            deferred = true;
        }
    }
    if deferred {
        if let Some(resident) = masks.iter_mut().flatten().next() {
            *resident = (0, upper);
        } else {
            masks[FAR] = (upper > 0).then_some((0, upper));
        }
    }
    masks
}

fn shared_material(
    cache: &mut HashMap<Mask, Handle<BarkMaterial>>,
    materials: &mut Assets<BarkMaterial>,
    models: &Models,
    (low, high): Mask,
) -> Handle<BarkMaterial> {
    cache
        .entry((low, high))
        .or_insert_with(|| {
            materials.add(dither_material(models.detail_material.clone(), low, high))
        })
        .clone()
}

fn spawn_detail(
    commands: &mut Commands,
    root: &PlanetRoot,
    frame: &ReferenceFrame,
    plant: Plant,
    (mesh, bounds): &(Handle<Mesh>, Aabb),
    material: Handle<BarkMaterial>,
) -> Entity {
    let (cell, translation) = frame.translation_to_grid(plant.position);
    root.spawn(
        commands,
        (
            cell,
            MaterialMeshBundle {
                mesh: mesh.clone(),
                material,
                transform: Transform::from_translation(translation)
                    .with_rotation(plant.rotation)
                    .with_scale(Vec3::splat(plant.scale)),
                ..default()
            },
            *bounds,
            NotShadowCaster,
            NotShadowReceiver,
        ),
    )
    .id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_thresholds_and_hysteresis_match_the_default_bands() {
        let bands = FernSettings::default().distances();
        assert_eq!(bands, [15.0, 35.0, 70.0, 120.0]);
        for (distance, lod) in [(0.0, 0), (15.0, 1), (35.0, 2), (70.0, 3), (120.0, 3)] {
            assert_eq!(choose_lod(distance, bands, 0.10, None), lod);
        }
        for (i, edge) in bands[..FAR].iter().copied().enumerate() {
            assert_eq!(choose_lod(edge * 1.08, bands, 0.10, Some(i)), i);
            assert_eq!(choose_lod(edge * 1.12, bands, 0.10, Some(i)), i + 1);
            assert_eq!(choose_lod(edge * 0.92, bands, 0.10, Some(i + 1)), i + 1);
            assert_eq!(choose_lod(edge * 0.88, bands, 0.10, Some(i + 1)), i);
        }
    }

    #[test]
    fn detail_and_card_masks_cover_the_same_root_during_handoff() {
        let plant = Plant {
            position: DVec3::Y * RADIUS,
            rotation: Quat::IDENTITY,
            scale: 1.0,
        };
        let mut fern = Fern::new(plant);
        fern.lod.update(2, 0.25);
        let masks = Fern::masks(&fern.lod, 12);
        assert_eq!(masks[FAR], Some((0, 9)));
        assert_eq!(masks[2], Some((9, 12)));
        fern.lod.update(FAR, 0.25);
        assert_eq!(Fern::masks(&fern.lod, 12)[FAR], Some((0, 12)));
    }

    #[test]
    fn exhausted_upload_budget_keeps_the_resident_representation() {
        let mut masks = [None; 4];
        masks[1] = Some((0, 8));
        masks[3] = Some((8, 12));
        assert_eq!(
            resident_masks(masks, &[None, None, None], 12),
            [None, None, None, Some((0, 12))]
        );
    }

    #[test]
    fn distant_mesh_uses_one_mask_index_per_fern() {
        let mut card = MeshBuilder::default();
        card.vertex(Vec3::ZERO, Vec3::Y, Vec2::ZERO, [1.0; 4]);
        card.vertex(Vec3::X, Vec3::Y, Vec2::X, [1.0; 4]);
        card.vertex(Vec3::Z, Vec3::Y, Vec2::Y, [1.0; 4]);
        card.triangle(0, 1, 2);
        let origin = DVec3::Y * RADIUS;
        let plants = [
            Plant {
                position: origin,
                rotation: Quat::IDENTITY,
                scale: 1.0,
            },
            Plant {
                position: origin + 4.0 * DVec3::Z,
                rotation: Quat::IDENTITY,
                scale: 1.0,
            },
        ];
        let mesh = batch_mesh(&card, origin, &plants);
        use bevy::render::mesh::VertexAttributeValues;
        let Some(VertexAttributeValues::Float32x2(slots)) = mesh.attribute(Mesh::ATTRIBUTE_UV_1)
        else {
            panic!("mask indices")
        };
        assert_eq!(
            slots.as_slice(),
            &[
                [0.0; 2],
                [0.0; 2],
                [0.0; 2],
                [1.0, 0.0],
                [1.0, 0.0],
                [1.0, 0.0]
            ]
        );
        assert_eq!(mesh.indices().unwrap().len(), 6);
    }
}

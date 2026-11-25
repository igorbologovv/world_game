//! Bounded ground-cover streaming, with two batched LODs at shared plant roots.

mod ecology;
mod plant_assets;

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use bevy::{
    asset::RenderAssetUsages,
    camera::primitives::{Aabb, MeshAabb},
    light::{NotShadowCaster, NotShadowReceiver},
    math::{DVec2, DVec3},
    mesh::Indices,
    prelude::*,
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
};
use world_core::{
    coordinates::{cube_direction, surface_uv},
    planet::RADIUS,
    runtime::{
        GridTransform, PlanetRoot, ReferenceFrame, ReferenceFrames, WorldCamera, WorldMovement,
    },
    SurfaceHeights,
};

use self::{
    ecology::Site,
    plant_assets::{Library, PlantAssets},
};
use super::{
    lod::{coverage, quantize, LodState},
    noise3, tangent_basis, FoliageMaterial, MeshBuilder, Random, Wind,
};

// A fixed face-cell count turns 24 m patches into 827 m tiles at Mars
// radius, making the 82 m cover-streaming range miss almost everything.
// Keep the patch size physical rather than tying it to planet diameter.
const PATCH_SIZE_METERS: f64 = 25.0;
const CELLS: f64 = 2.0 * RADIUS / PATCH_SIZE_METERS;
const FADE_START: f32 = 44.0;
const FADE_END: f32 = 62.0;
const PRELOAD: f64 = 82.0;
const MAX_PATCHES: usize = 78;
const PATCHES_PER_TICK: usize = 2;
const PATCH_JOBS: usize = 4;
const PATCH_UPLOADS_PER_FRAME: usize = 2;
const DETAIL_LOADS_PER_FRAME: usize = 1;
const MAX_DETAIL_PATCHES: usize = 8;
const DETAIL_IN: f32 = 17.0;
const DETAIL_OUT: f32 = 22.0;
const MAX_BACKGROUND_TRIANGLES: usize = 1850;
const MAX_DETAIL_TRIANGLES: usize = 9600;
const MAX_DETAILED_PLANTS: usize = 8;
const SAMPLE_GRID: u32 = 10;
const WIND_MARGIN: f32 = 0.20;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct Patch {
    face: usize,
    x: u32,
    y: u32,
}

impl Patch {
    fn at(direction: DVec3) -> Self {
        let (face, uv) = surface_uv(direction);
        let cell = (uv * CELLS).min(DVec2::splat(CELLS - 1.0)).as_uvec2();
        Self {
            face,
            x: cell.x,
            y: cell.y,
        }
    }

    fn direction(self, offset: DVec2) -> DVec3 {
        let uv = (DVec2::new(f64::from(self.x), f64::from(self.y)) + offset) / CELLS;
        cube_direction(self.face, uv)
    }

    fn seed(self) -> u64 {
        (self.face as u64 * 1_002_407)
            ^ (u64::from(self.x) * 73_856_093)
            ^ (u64::from(self.y) * 19_349_663)
    }
}

#[derive(Resource, Default)]
struct GroundCover {
    patches: HashMap<Patch, PatchState>,
    order: Vec<Patch>,
    next_tick: f64,
}

enum PatchState {
    Queued(Task<Option<PatchMeshes>>),
    Empty,
    Resident(Box<GroundPatch>),
}

struct PatchMeshes {
    compact: Mesh,
    detail: Option<Mesh>,
    origin: DVec3,
    rotation: Quat,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Compact,
    Detail,
}

struct GroundPatch {
    compact: Entity,
    detail: Option<Entity>,
    detail_mesh: Option<Mesh>,
    origin: DVec3,
    rotation: Quat,
    arrival: f32,
    lod: LodState<Level>,
    center: DVec3,
    radius: f64,
}

#[derive(Resource)]
struct GroundMaterial {
    outer_fade: [Handle<FoliageMaterial>; 13],
    intervals: [[Option<Handle<FoliageMaterial>>; 13]; 13],
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<GroundCover>()
        .add_systems(Startup, (build_material, plant_assets::request))
        .add_systems(
            Update,
            (plant_assets::finish, stream).chain().after(WorldMovement),
        );
}

fn build_material(mut commands: Commands, mut materials: ResMut<Assets<FoliageMaterial>>) {
    let base = StandardMaterial {
        perceptual_roughness: 0.91,
        reflectance: 0.12,
        diffuse_transmission: 0.29,
        double_sided: true,
        cull_mode: None,
        ..default()
    };
    let mut add = |low, high, start, end| {
        materials.add(FoliageMaterial {
            base: base.clone(),
            extension: Wind {
                coverage: Vec4::new(coverage(low), coverage(high), start, end),
                ..default()
            },
        })
    };
    let outer_fade = std::array::from_fn(|step| add(0, step as u8, FADE_START, FADE_END));
    let intervals = std::array::from_fn(|low| {
        std::array::from_fn(|high| (low < high).then(|| add(low as u8, high as u8, 0.0, 0.0)))
    });
    commands.insert_resource(GroundMaterial {
        outer_fade,
        intervals,
    });
}

fn stream(
    mut commands: Commands,
    time: Res<Time>,
    mut cover: ResMut<GroundCover>,
    mut meshes: ResMut<Assets<Mesh>>,
    material: Res<GroundMaterial>,
    library: Res<PlantAssets>,
    heights: Res<SurfaceHeights>,
    cameras: Query<(Entity, GridTransform), With<WorldCamera>>,
    frames: ReferenceFrames,
    root: Res<PlanetRoot>,
    mut renderables: Query<(&mut MeshMaterial3d<FoliageMaterial>, &mut Visibility)>,
) {
    let PlantAssets::Ready(library) = &*library else {
        return;
    };
    let Ok((camera_entity, camera)) = cameras.single() else {
        return;
    };
    let Some(frame) = frames.parent_frame(camera_entity) else {
        return;
    };
    let eye = frame.grid_position_double(&camera.cell, &camera.transform);
    let now = time.elapsed_secs_f64();
    let discover = now >= cover.next_tick;
    let GroundCover {
        patches,
        order,
        next_tick,
    } = &mut *cover;
    if discover {
        *next_tick = now + 0.12;
        *order = eye
            .try_normalize()
            .filter(|&up| {
                eye.length() - RADIUS - heights.sample(up).unwrap_or(0.0) <= PRELOAD + 22.0
            })
            .map_or_else(Vec::new, visible_patches);
        let wanted: HashSet<_> = order.iter().copied().collect();
        let retiring: Vec<_> = patches
            .keys()
            .copied()
            .filter(|key| !wanted.contains(key))
            .take(6)
            .collect();
        for key in retiring {
            if let Some(PatchState::Resident(old)) = patches.remove(&key) {
                commands.entity(old.compact).despawn();
                if let Some(detail) = old.detail {
                    commands.entity(detail).despawn();
                }
            }
        }
    }

    let mut uploads = PATCH_UPLOADS_PER_FRAME;
    let mut detail_loads = DETAIL_LOADS_PER_FRAME;
    let mut details = patches
        .values()
        .filter(|state| matches!(state, PatchState::Resident(patch) if patch.detail.is_some()))
        .count();
    let mut update = PatchUpdate {
        commands: &mut commands,
        root: &root,
        frame,
        meshes: &mut meshes,
        material: &material,
    };
    // Cached patches also update when flying away, so old detail cannot linger
    // beyond its fade while retirements are being budgeted.
    for state in patches.values_mut() {
        if let PatchState::Resident(patch) = state {
            let distance = eye.distance(patch.center) as f32;
            if patch.detail.is_some() && distance > FADE_START {
                update
                    .commands
                    .entity(patch.detail.take().unwrap())
                    .despawn();
                patch.lod = LodState::default();
                details -= 1;
            }
            if distance > FADE_END + patch.radius as f32 {
                if let Ok((_, mut visibility)) = renderables.get_mut(patch.compact) {
                    visibility.set_if_neq(Visibility::Hidden);
                }
            }
        }
    }
    let dt = time.delta_secs().min(0.075);
    for key in order.iter() {
        let Some(state) = patches.get_mut(key) else {
            continue;
        };
        match state {
            PatchState::Queued(task) if uploads > 0 => {
                if let Some(result) = block_on(poll_once(task)) {
                    *state = if let Some(patch) = result {
                        uploads -= 1;
                        PatchState::Resident(Box::new(update.upload(patch)))
                    } else {
                        PatchState::Empty
                    };
                }
            }
            PatchState::Resident(patch) => {
                let admit = details < MAX_DETAIL_PATCHES && detail_loads > 0;
                let delta = update.select_lod(patch, eye, dt, admit, &mut renderables);
                if delta > 0 {
                    detail_loads -= 1;
                }
                details = details.saturating_add_signed(delta);
            }
            _ => {}
        }
    }
    if !discover {
        return;
    }
    let queued = patches
        .values()
        .filter(|state| matches!(state, PatchState::Queued(_)))
        .count();
    let available = MAX_PATCHES
        .saturating_sub(patches.len())
        .min(PATCH_JOBS.saturating_sub(queued))
        .min(PATCHES_PER_TICK);
    let pool = AsyncComputeTaskPool::get();
    let mut remaining = available;
    for &key in order.iter() {
        if remaining == 0 {
            break;
        }
        patches.entry(key).or_insert_with(|| {
            remaining -= 1;
            let heights = heights.clone();
            let library = Arc::clone(library);
            PatchState::Queued(pool.spawn(async move { load_patch(key, &heights, &library) }))
        });
    }
}

struct PatchUpdate<'a, 'w, 's> {
    commands: &'a mut Commands<'w, 's>,
    root: &'a PlanetRoot,
    frame: &'a ReferenceFrame,
    meshes: &'a mut Assets<Mesh>,
    material: &'a GroundMaterial,
}

impl PatchUpdate<'_, '_, '_> {
    fn spawn(&mut self, mut mesh: Mesh, origin: DVec3, rotation: Quat, bounds: Aabb) -> Entity {
        let (cell, translation) = self.frame.translation_to_grid(origin);
        mesh.asset_usage = RenderAssetUsages::RENDER_WORLD;
        self.root
            .spawn(
                self.commands,
                (
                    cell,
                    Mesh3d(self.meshes.add(mesh)),
                    MeshMaterial3d(self.material.outer_fade[0].clone()),
                    Transform::from_translation(translation).with_rotation(rotation),
                    bounds,
                    Visibility::Hidden,
                    NotShadowCaster,
                    NotShadowReceiver,
                ),
            )
            .id()
    }

    fn upload(&mut self, patch: PatchMeshes) -> GroundPatch {
        let PatchMeshes {
            compact,
            detail,
            origin,
            rotation,
        } = patch;
        let mut bounds = compact.get_aabb().expect("nonempty compact plant patch");
        bounds.half_extents += WIND_MARGIN;
        let center = origin + (rotation * Vec3::from(bounds.center)).as_dvec3();
        let compact = self.spawn(compact, origin, rotation, bounds);
        GroundPatch {
            compact,
            detail: None,
            detail_mesh: detail,
            origin,
            rotation,
            arrival: 0.0,
            lod: LodState::default(),
            center,
            radius: f64::from(bounds.half_extents.length()),
        }
    }

    fn select_lod(
        &mut self,
        patch: &mut GroundPatch,
        eye: DVec3,
        dt: f32,
        admit: bool,
        renderables: &mut Query<(&mut MeshMaterial3d<FoliageMaterial>, &mut Visibility)>,
    ) -> isize {
        let distance = eye.distance(patch.center) as f32;
        let boundary = if patch.lod.desired == Some(Level::Detail) {
            DETAIL_OUT
        } else {
            DETAIL_IN
        };
        let mut delta = 0;
        if distance < boundary && admit && patch.detail.is_none() {
            if let Some(mesh) = &patch.detail_mesh {
                let mut bounds = mesh.get_aabb().expect("nonempty detailed plant patch");
                bounds.half_extents += WIND_MARGIN;
                patch.detail = Some(self.spawn(mesh.clone(), patch.origin, patch.rotation, bounds));
                delta += 1;
            }
        }
        let target = if patch.detail.is_some() && distance < boundary {
            Level::Detail
        } else {
            Level::Compact
        };
        patch.lod.update(target, dt * 4.5);
        patch.arrival = (patch.arrival + dt * 3.5).min(1.0);
        let in_range = distance <= FADE_END + patch.radius as f32;
        let arrival = quantize(patch.arrival);
        let slots = patch
            .lod
            .slots(arrival)
            .expect("ground patch has a current LOD");
        for (level, entity) in [
            (Level::Compact, Some(patch.compact)),
            (Level::Detail, patch.detail),
        ] {
            let Some(entity) = entity else {
                continue;
            };
            let Some((_, low, high)) = slots
                .iter()
                .copied()
                .find(|&(choice, low, high)| choice == level && low < high && in_range)
            else {
                if let Ok((_, mut visibility)) = renderables.get_mut(entity) {
                    visibility.set_if_neq(Visibility::Hidden);
                }
                continue;
            };
            let material = if level == Level::Compact && !patch.lod.is_blending() {
                &self.material.outer_fade[high as usize]
            } else {
                self.material.intervals[low as usize][high as usize]
                    .as_ref()
                    .unwrap()
            };
            // A newly admitted batch is still in Commands until the deferred flush.
            if let Ok((mut handle, mut visibility)) = renderables.get_mut(entity) {
                handle.set_if_neq(MeshMaterial3d(material.clone()));
                visibility.set_if_neq(Visibility::Inherited);
            }
        }
        if target == Level::Compact
            && patch.lod.current == Some(Level::Compact)
            && !patch.lod.is_blending()
        {
            if let Some(entity) = patch.detail.take() {
                self.commands.entity(entity).despawn();
                delta -= 1;
            }
        }
        delta
    }
}

fn visible_patches(up: DVec3) -> Vec<Patch> {
    let (east, north) = tangent_basis(up);
    let spacing = 13.0 / RADIUS;
    let mut candidates = HashSet::new();
    for y in -7..=7 {
        for x in -7..=7 {
            candidates.insert(Patch::at(
                up + spacing * (east * f64::from(x) + north * f64::from(y)),
            ));
        }
    }
    let mut sorted: Vec<_> = candidates
        .into_iter()
        .filter_map(|key| {
            let distance = key.direction(DVec2::splat(0.5)).distance_squared(up) * RADIUS.powi(2);
            (distance <= PRELOAD * PRELOAD).then_some((distance, key))
        })
        .collect();
    sorted.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    sorted
        .into_iter()
        .take(MAX_PATCHES)
        .map(|(_, key)| key)
        .collect()
}

fn load_patch(key: Patch, heights: &SurfaceHeights, library: &Library) -> Option<PatchMeshes> {
    let up = key.direction(DVec2::splat(0.5));
    let patch_height = heights.sample(up)?;
    let origin = up * (RADIUS + patch_height);
    let rotation = Quat::from_rotation_arc(Vec3::Y, up.as_vec3());
    let inverse = rotation.inverse();
    let sites: Vec<_> = ecology::layout(key, heights, origin)
        .into_iter()
        .filter_map(|site| library.get(site).map(|model| (site, model)))
        .collect();
    let background = background(key, heights, origin, inverse, &sites);
    let mut compact = background.clone();
    let mut detail = background;
    let mut remaining = MAX_DETAIL_TRIANGLES.saturating_sub(
        detail.indices.len() / 3
            + sites
                .iter()
                .map(|(_, model)| model.compact.triangles())
                .sum::<usize>(),
    );
    let mut upgrades = 0;
    for (site, model) in sites {
        model.compact.append(&mut compact, site, origin, inverse);
        let difference = model
            .detail
            .triangles()
            .saturating_sub(model.compact.triangles());
        let chosen = if difference > 0 && difference <= remaining && upgrades < MAX_DETAILED_PLANTS
        {
            remaining -= difference;
            upgrades += 1;
            &model.detail
        } else {
            &model.compact
        };
        chosen.append(&mut detail, site, origin, inverse);
    }
    if compact.indices.is_empty() {
        return None;
    }
    Some(PatchMeshes {
        compact: finish_patch(compact),
        detail: (upgrades > 0).then(|| finish_patch(detail)),
        origin,
        rotation,
    })
}

fn finish_patch(mut mesh: MeshBuilder) -> Mesh {
    // The entire detailed patch can have at most 9,600 triangles, so all
    // referenced vertices fit into a 16-bit index even without sharing.
    debug_assert!(mesh.positions.len() < usize::from(u16::MAX));
    let indices = std::mem::take(&mut mesh.indices)
        .into_iter()
        .map(|i| i as u16)
        .collect();
    let mut mesh = mesh.finish();
    mesh.insert_indices(Indices::U16(indices));
    mesh
}

fn background(
    key: Patch,
    heights: &SurfaceHeights,
    origin: DVec3,
    inverse: Quat,
    sites: &[(Site, &plant_assets::Model)],
) -> MeshBuilder {
    let up = origin.normalize();
    let patch_height = origin.length() - RADIUS;
    let (east, north) = tangent_basis(up);
    let wet = 0.51 + noise3(up.as_vec3() * 5.8 + Vec3::new(17.3, -4.7, 11.8));
    let temperature = 1.07 - 0.78 * up.y.abs() as f32 - patch_height.max(0.0) as f32 / 16_000.0;
    let mut mesh = MeshBuilder::default();
    let mut rng = Random::new(key.seed());
    for y in 0..SAMPLE_GRID {
        for x in 0..SAMPLE_GRID {
            // Leave enough room for one legacy plant, including a fern.
            if mesh.indices.len() / 3 + 216 > MAX_BACKGROUND_TRIANGLES {
                return mesh;
            }
            let choice = rng.unit();
            let offset = (DVec2::new(f64::from(x) + rng.unit64(), f64::from(y) + rng.unit64()))
                / f64::from(SAMPLE_GRID);
            let direction = key.direction(offset);
            let Some(elevation) = heights.sample(direction) else {
                continue;
            };
            if !(1.35..3200.0).contains(&elevation)
                || heights.open_water_at(direction, elevation).is_some()
            {
                continue;
            }
            let slope_step = 3.0 / RADIUS;
            let east_height = heights
                .sample((direction + east * slope_step).normalize())
                .unwrap_or(elevation);
            let north_height = heights
                .sample((direction + north * slope_step).normalize())
                .unwrap_or(elevation);
            if (east_height - elevation).hypot(north_height - elevation) > 2.15 {
                continue;
            }
            let bank = elevation < 9.0
                || (choice < 0.22
                    && [east, north, -east, -north].into_iter().any(|axis| {
                        heights.is_river((direction + axis * (9.0 / RADIUS)).normalize())
                    }));
            let ground = direction * (RADIUS + elevation);
            if sites
                .iter()
                .any(|(site, _)| site.position.distance_squared(ground) < 1.05_f64.powi(2))
            {
                continue;
            }
            let cover = noise3(ground.as_vec3() * 0.21 + Vec3::new(4.1, 11.8, -7.3));
            let density = (0.49 + 0.44 * wet + 1.34 * cover).clamp(0.12, 0.96);
            let position = direction * (RADIUS + elevation - 0.13);
            let root = inverse * (position - origin).as_vec3();
            let plant = Plant {
                root,
                phase: rng.range(0.0, std::f32::consts::TAU),
            };
            if choice > density || (temperature < 0.16 && choice > 0.18) {
                if choice < density + 0.09 {
                    plant.gravel(&mut mesh, &mut rng, wet);
                }
            } else if bank {
                plant.grass(&mut mesh, &mut rng, true, wet);
            } else if sites.is_empty() && temperature > 0.33 && wet > 0.43 && choice < 0.18 {
                plant.fern(&mut mesh, &mut rng);
            } else if sites.is_empty() && temperature > 0.24 && choice < 0.27 {
                plant.flowers(&mut mesh, &mut rng);
            } else {
                plant.grass(&mut mesh, &mut rng, false, wet);
            }
        }
    }
    mesh
}

struct Plant {
    root: Vec3,
    phase: f32,
}

impl Plant {
    fn gravel(&self, mesh: &mut MeshBuilder, rng: &mut Random, wet: f32) {
        let tone = Vec3::new(0.22, 0.20, 0.16).lerp(Vec3::new(0.087, 0.095, 0.076), wet);
        for rock in 0..3 {
            let angle = self.phase + rock as f32 * 2.39996;
            let (sin, cos) = angle.sin_cos();
            let center = self.root + Vec3::new(cos, 0.0, sin) * rng.range(0.03, 0.27);
            let radius = rng.range(0.075, 0.26);
            let hue = tone * rng.range(0.83, 1.19);
            let mut vertex = |point: Vec3, normal: Vec3, light: f32| {
                let color = hue * light;
                mesh.wind.push([0.0; 2]);
                mesh.vertex(point, normal, Vec2::ZERO, [color.x, color.y, color.z, 1.0])
            };
            let top = vertex(
                center + Vec3::new(-radius * 0.18, 0.13 + radius * 0.52, radius * 0.12),
                Vec3::Y,
                1.12,
            );
            let rim: [u32; 5] = std::array::from_fn(|index| {
                let angle = angle + index as f32 * std::f32::consts::TAU / 5.0;
                let (sin, cos) = angle.sin_cos();
                let axis = Vec3::new(cos, 0.0, sin);
                let point = center
                    + axis * radius * rng.range(0.69, 1.14)
                    + Vec3::Y * rng.range(0.08, 0.15);
                vertex(point, axis + Vec3::Y * 0.5, rng.range(0.69, 0.98))
            });
            for i in 0..rim.len() {
                mesh.triangle(top, rim[(i + 1) % rim.len()], rim[i]);
            }
        }
    }

    fn vertex(&self, mesh: &mut MeshBuilder, point: Vec3, normal: Vec3, tint: Vec3) -> u32 {
        mesh.wind
            .push([(point.y * 0.11).clamp(0.0, 0.17), self.phase]);
        mesh.vertex(
            self.root + point,
            normal,
            Vec2::ZERO,
            [tint.x, tint.y, tint.z, 1.0],
        )
    }

    fn ribbon(
        &self,
        mesh: &mut MeshBuilder,
        start: Vec3,
        tip: Vec3,
        half_width: Vec3,
        color: Vec3,
    ) {
        let delta = tip - start;
        let middle = start + delta * 0.46 + Vec3::Y * delta.length() * 0.21;
        let normal = half_width.cross(delta).normalize_or_zero();
        let a = self.vertex(mesh, start - half_width * 0.55, normal, color * 0.43);
        let b = self.vertex(mesh, start + half_width * 0.55, normal, color * 0.50);
        let c = self.vertex(mesh, middle + half_width, normal, color * 0.90);
        let d = self.vertex(mesh, middle - half_width, normal, color * 0.76);
        let e = self.vertex(mesh, tip, normal, color * 1.20);
        mesh.triangle(a, b, d);
        mesh.triangle(b, c, d);
        mesh.triangle(d, c, e);
    }

    fn grass(&self, mesh: &mut MeshBuilder, rng: &mut Random, reed: bool, wet: f32) {
        let color =
            Vec3::new(0.18, 0.22, 0.043).lerp(Vec3::new(0.068, 0.23, 0.046), wet.clamp(0.0, 1.0));
        for blade in 0..if reed { 8 } else { 6 } {
            let angle = self.phase + blade as f32 * 2.39996 + rng.range(-0.28, 0.28);
            let (sin, cos) = angle.sin_cos();
            let direction = Vec3::new(cos, 0.0, sin);
            let start = direction * rng.range(0.03, 0.24);
            let height = if reed {
                rng.range(0.75, 1.52)
            } else {
                rng.range(0.28, 0.67)
            };
            let tip = start + Vec3::Y * height + direction * rng.range(0.08, 0.47);
            let tint = color * rng.range(0.73, 1.36);
            self.ribbon(
                mesh,
                start,
                tip,
                Vec3::Y.cross(direction) * rng.range(0.018, 0.044),
                tint,
            );
            if reed && blade % 3 == 0 {
                let seed = Vec3::new(0.22, 0.125, 0.036);
                let head = tip + Vec3::Y * 0.18;
                self.ribbon(mesh, tip - Vec3::Y * 0.08, head, direction * 0.039, seed);
                self.ribbon(
                    mesh,
                    tip - Vec3::Y * 0.08,
                    head,
                    direction.cross(Vec3::Y) * 0.039,
                    seed,
                );
            }
        }
    }

    fn fern(&self, mesh: &mut MeshBuilder, rng: &mut Random) {
        for frond in 0..6 {
            let angle = self.phase + frond as f32 * 2.39996;
            let (sin, cos) = angle.sin_cos();
            let forward = Vec3::new(cos, 0.0, sin);
            let side = Vec3::Y.cross(forward);
            let length = rng.range(0.57, 1.06);
            let green = Vec3::new(0.069, 0.28, 0.058) * rng.range(0.75, 1.22);
            let shape =
                |t: f32| forward * length * t + Vec3::Y * (0.22 + 1.9 * t - 1.36 * t * t) * length;
            self.ribbon(mesh, Vec3::ZERO, shape(0.64), side * 0.023, green);
            for row in 1..=5 {
                let t = row as f32 / 6.0;
                let spread = (std::f32::consts::PI * t).sin() * length * 0.28;
                for sign in [-1.0, 1.0] {
                    let from = shape(t);
                    let to = shape((t + 0.18).min(1.0)) + side * spread * sign;
                    self.ribbon(mesh, from, to, forward * spread * 0.18, green);
                }
            }
            self.ribbon(mesh, shape(0.62), shape(1.0), side * 0.05, green * 1.16);
        }
    }

    fn flowers(&self, mesh: &mut MeshBuilder, rng: &mut Random) {
        let color = match rng.next_u32() % 4 {
            0 => Vec3::new(0.72, 0.53, 0.13),
            1 => Vec3::new(0.48, 0.21, 0.64),
            2 => Vec3::new(0.77, 0.75, 0.61),
            _ => Vec3::new(0.63, 0.26, 0.35),
        };
        let green = Vec3::new(0.084, 0.23, 0.032);
        for stem in 0..4 {
            let angle = self.phase + stem as f32 * 2.4;
            let (sin, cos) = angle.sin_cos();
            let side = Vec3::new(cos, 0.0, sin);
            let start = side * rng.range(0.02, 0.22);
            let bloom = start + Vec3::Y * rng.range(0.40, 0.77) + side * 0.11;
            self.ribbon(mesh, start, bloom, side * 0.019, green);
            let leaf = start.lerp(bloom, 0.48);
            self.ribbon(
                mesh,
                leaf,
                leaf + side * 0.23 + Vec3::Y * 0.13,
                Vec3::Y.cross(side) * 0.042,
                green,
            );
            let center = self.vertex(
                mesh,
                bloom + Vec3::Y * 0.027,
                Vec3::Y,
                Vec3::new(0.65, 0.33, 0.055),
            );
            for petal in 0..5 {
                let angle = self.phase + petal as f32 * std::f32::consts::TAU / 5.0;
                let (sin, cos) = angle.sin_cos();
                let axis = Vec3::new(cos, 0.0, sin);
                let spread = axis.cross(Vec3::Y) * 0.044;
                let left = self.vertex(mesh, bloom + axis * 0.085 - spread, Vec3::Y, color * 0.82);
                let tip = self.vertex(mesh, bloom + axis * 0.125 + Vec3::Y * 0.034, Vec3::Y, color);
                let right = self.vertex(mesh, bloom + axis * 0.085 + spread, Vec3::Y, color * 0.94);
                mesh.triangle(center, tip, left);
                mesh.triangle(center, right, tip);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_cover_patch_size_is_local_at_mars_radius() {
        let meters = 2.0 * RADIUS / CELLS;
        assert!((meters - PATCH_SIZE_METERS).abs() < 0.0001);
        let up = DVec3::new(0.2, 0.92, -0.3).normalize();
        let patches = visible_patches(up);
        assert!(
            !patches.is_empty(),
            "ground cover must stream near any camera location"
        );
        assert!(patches.len() <= MAX_PATCHES);
    }

    #[test]
    fn cover_patches_are_bounded_deterministic_and_cross_cube_seams() {
        let eye = DVec3::new(1.0, 0.82, 1.0).normalize();
        let patches = visible_patches(eye);
        assert_eq!(patches, visible_patches(eye));
        assert!(patches.len() <= MAX_PATCHES);
        assert!(patches.iter().any(|patch| patch.face == 1));
        assert!(patches.iter().any(|patch| patch.face == 3));
    }

    #[test]
    fn ground_cover_is_generated_near_the_starting_explorer() {
        let heights = SurfaceHeights::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets"
        )));
        let up = world_core::coordinates::terrain_aligned_direction(
            world_regions::STARTER_REGION.spawn.direction,
        );
        assert!(visible_patches(up)
            .into_iter()
            .take(8)
            .any(|patch| load_patch(patch, &heights, &Library::default()).is_some()));
    }

    #[test]
    fn plant_blades_have_wind_weights_and_bounded_geometry() {
        let mut mesh = MeshBuilder::default();
        let plant = Plant {
            root: Vec3::ZERO,
            phase: 0.83,
        };
        let mut rng = Random::new(237);
        plant.fern(&mut mesh, &mut rng);
        plant.flowers(&mut mesh, &mut rng);
        plant.grass(&mut mesh, &mut rng, true, 0.7);
        assert_eq!(mesh.wind.len(), mesh.positions.len());
        assert!(mesh.indices.len() / 3 < 350);
        assert!(mesh
            .wind
            .iter()
            .all(|weight| (0.0..=WIND_MARGIN).contains(&weight[0])));
    }

    #[test]
    fn client_streams_the_plant_library_and_retires_both_patch_lods() {
        use bevy::{
            asset::AssetPlugin, gltf::GltfPlugin, mesh::MeshPlugin, time::TimeUpdateStrategy,
            world_serialization::WorldSerializationPlugin,
        };
        use std::{
            path::Path,
            time::{Duration, Instant},
        };
        use world_core::{
            coordinates::terrain_aligned_direction,
            runtime::{WorldAssets, WorldSpacePlugin},
        };

        let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin {
                file_path: path.to_string_lossy().into_owned(),
                ..default()
            },
            WorldSerializationPlugin,
            MeshPlugin,
            GltfPlugin::default(),
            WorldSpacePlugin::new(WorldAssets::new(path.into())),
        ))
        .init_asset::<FoliageMaterial>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            40,
        )));
        install(&mut app);
        app.finish();
        app.cleanup();
        app.update();

        let up = terrain_aligned_direction(world_regions::PALM_COAST_REGION.spawn.direction);
        let world = app.world_mut();
        let ground = world.resource::<SurfaceHeights>().sample(up).unwrap();
        let root = world.resource::<PlanetRoot>().entity();
        let (cell, translation) = world
            .get::<ReferenceFrame>(root)
            .unwrap()
            .translation_to_grid(up * (RADIUS + ground + 1.7));
        let camera = world
            .spawn((
                WorldCamera::FirstPerson,
                cell,
                Transform::from_translation(translation),
                ChildOf(root),
            ))
            .id();

        let start = Instant::now();
        loop {
            app.update();
            let cover = app.world().resource::<GroundCover>();
            assert!(cover.patches.len() <= MAX_PATCHES);
            assert!(
                cover
                    .patches
                    .values()
                    .filter(|state| matches!(state, PatchState::Queued(_)))
                    .count()
                    <= PATCH_JOBS
            );
            assert!(cover.patches.values().filter(|state| matches!(state, PatchState::Resident(patch) if patch.detail.is_some())).count() <= MAX_DETAIL_PATCHES);
            if cover.patches.values().any(|state| matches!(state, PatchState::Resident(patch) if patch.arrival == 1.0 && patch.lod.current == Some(Level::Detail))) {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "ground plants must stream near the camera"
            );
            std::thread::sleep(Duration::from_millis(1));
        }

        let world = app.world_mut();
        let PlantAssets::Ready(library) = &*world.resource::<PlantAssets>() else {
            panic!("catalog should be ready");
        };
        for species in 0..ecology::SPECIES.len() {
            for variant in 0..ecology::VARIANTS {
                let model = library
                    .get(Site {
                        species,
                        variant,
                        position: DVec3::ZERO,
                        rotation: Quat::IDENTITY,
                        scale: Vec3::ONE,
                        phase: 0.0,
                    })
                    .expect("every shipped species and variant must load both meshes");
                assert!(model.compact.triangles() <= plant_assets::COMPACT_TRIANGLES);
                assert!(model.detail.triangles() > model.compact.triangles());
            }
        }

        let batches: Vec<_> = world
            .resource::<GroundCover>()
            .patches
            .values()
            .filter_map(|state| {
                let PatchState::Resident(patch) = state else {
                    return None;
                };
                if let Some(mesh) = &patch.detail_mesh {
                    assert!(mesh.indices().unwrap().len() / 3 <= MAX_DETAIL_TRIANGLES);
                }
                Some([Some(patch.compact), patch.detail])
            })
            .flatten()
            .flatten()
            .collect();
        assert!(batches
            .iter()
            .all(|&entity| world.get::<ChildOf>(entity).unwrap().parent() == root));
        assert!(batches
            .iter()
            .any(|&entity| world.get::<Visibility>(entity) == Some(&Visibility::Inherited)));

        let (key, compact, detail) = world
            .resource::<GroundCover>()
            .patches
            .iter()
            .find_map(|(&key, state)| match state {
                PatchState::Resident(patch) if patch.lod.current == Some(Level::Detail) => {
                    Some((key, patch.compact, patch.detail.unwrap()))
                }
                _ => None,
            })
            .unwrap();
        let (cell, translation) = world
            .get::<ReferenceFrame>(root)
            .unwrap()
            .translation_to_grid(up * (RADIUS + ground + 32.0));
        world
            .entity_mut(camera)
            .insert((cell, Transform::from_translation(translation)));
        for _ in 0..10 {
            app.update();
        }
        let world = app.world_mut();
        let PatchState::Resident(patch) = &world.resource::<GroundCover>().patches[&key] else {
            panic!("compact patch should stay resident at the same root");
        };
        assert!(patch.lod.current == Some(Level::Compact) && patch.detail.is_none());
        assert_eq!(patch.compact, compact);
        assert_eq!(
            world.get::<Visibility>(compact),
            Some(&Visibility::Inherited)
        );
        assert!(world.get_entity(detail).is_err());

        let (cell, translation) = world
            .get::<ReferenceFrame>(root)
            .unwrap()
            .translation_to_grid(up * (RADIUS + ground + 1100.0));
        world
            .entity_mut(camera)
            .insert((cell, Transform::from_translation(translation)));
        for _ in 0..60 {
            app.update();
        }
        assert!(app.world().resource::<GroundCover>().patches.is_empty());
        assert!(batches
            .into_iter()
            .all(|entity| app.world().get_entity(entity).is_err()));
    }
}

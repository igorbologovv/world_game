//! Spatially batched inland water with camera-dependent spherical tessellation.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use bevy::{
    asset::RenderAssetUsages,
    gizmos::config::GizmoConfigGroup,
    light::NotShadowCaster,
    math::{DVec3, IVec3},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    reflect::TypePath,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
};
use world_core::{
    planet::RADIUS,
    runtime::{
        Explorer, GridTransform, PlanetRoot, ReferenceFrames, WorldCamera, WorldMovement,
        WorldReady,
    },
    LakeTile, RiverReach, SurfaceHeights,
};

const PATCH_SIZE: f64 = 65_536.0;
const NEAR_RANGE: f64 = 32_000.0;
const LOD_HYSTERESIS: f64 = 9_000.0;
const MAX_JOBS: usize = 4;
const MAX_UPLOADS: usize = 4;

#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub(super) struct RiverMaterial {
    #[texture(0)]
    #[sampler(1)]
    pub(super) ripples: Handle<Image>,
}

impl Material for RiverMaterial {
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn vertex_shader() -> ShaderRef {
        "shaders/river_water.wesl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/river_water.wesl".into()
    }
}

#[derive(Resource)]
pub(super) struct InlandMaterial(pub(super) Handle<RiverMaterial>);

#[derive(Default, Reflect, GizmoConfigGroup)]
pub(super) struct RiverMapLines;

#[derive(Default)]
struct Patch {
    rivers: Vec<usize>,
    lakes: Vec<usize>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Detail {
    Near,
    Far,
}

#[derive(Resource, Default)]
struct InlandPatches {
    catalog_task: Option<Task<HashMap<IVec3, Arc<Patch>>>>,
    catalog: HashMap<IVec3, Arc<Patch>>,
    desired: HashMap<IVec3, Detail>,
    queue: VecDeque<(IVec3, Detail)>,
    active: HashMap<IVec3, (Detail, Entity, Handle<Mesh>)>,
    pending: HashMap<IVec3, (Detail, Task<Mesh>)>,
    eye: Option<DVec3>,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<InlandPatches>()
        .add_systems(Startup, request.after(WorldReady))
        .add_systems(Update, stream.after(WorldMovement));
}

fn request(
    heights: Res<SurfaceHeights>,
    mut patches: ResMut<InlandPatches>,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<RiverMaterial>>,
    mut commands: Commands,
) {
    use bevy::image::{
        ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler,
        ImageSamplerDescriptor,
    };
    let ripples = asset_server
        .load_builder()
        .with_settings(|settings: &mut ImageLoaderSettings| {
            settings.is_srgb = false;
            settings.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                mipmap_filter: ImageFilterMode::Linear,
                ..ImageSamplerDescriptor::linear()
            });
        })
        .load("textures/water/freshwater.ktx2");
    commands.insert_resource(InlandMaterial(materials.add(RiverMaterial { ripples })));

    let heights = heights.clone();
    patches.catalog_task = Some(AsyncComputeTaskPool::get().spawn(async move {
        let mut catalog = HashMap::<_, Patch>::new();
        let patch = |up: DVec3| (up * (RADIUS / PATCH_SIZE)).floor().as_ivec3();
        for (index, reach) in heights.river_reaches().iter().enumerate() {
            catalog
                .entry(patch(reach.direction(0.5, 0.0)))
                .or_default()
                .rivers
                .push(index);
        }
        for (index, tile) in heights.lake_tiles().iter().enumerate() {
            let center = tile.corners.iter().sum::<DVec3>().normalize();
            catalog.entry(patch(center)).or_default().lakes.push(index);
        }
        catalog
            .into_iter()
            .map(|(key, data)| (key, Arc::new(data)))
            .collect()
    }));
}

fn anchor(key: IVec3) -> DVec3 {
    (key.as_dvec3() + DVec3::splat(0.5)) * PATCH_SIZE
}

fn stream(
    mut commands: Commands,
    mut state: ResMut<InlandPatches>,
    mut meshes: ResMut<Assets<Mesh>>,
    heights: Res<SurfaceHeights>,
    material: Res<InlandMaterial>,
    cameras: Query<(Entity, GridTransform), With<WorldCamera>>,
    frames: ReferenceFrames,
    root: Res<PlanetRoot>,
) {
    let Ok((entity, camera)) = cameras.single() else {
        return;
    };
    let Some(frame) = frames.parent_frame(entity) else {
        return;
    };
    let eye = frame.grid_position_double(&camera.cell, &camera.transform);
    if let Some(task) = &mut state.catalog_task {
        let Some(catalog) = block_on(poll_once(task)) else {
            return;
        };
        state.catalog = catalog;
        state.catalog_task = None;
    }

    if state
        .eye
        .is_none_or(|last| last.distance_squared(eye) > 3500.0_f64.powi(2))
    {
        state.eye = Some(eye);
        // Include the full patch plus mountain and lake elevation in the
        // horizon margin. Bevy still does per-patch frustum culling.
        let horizon = (eye.length_squared() - RADIUS * RADIUS).max(0.0).sqrt();
        let range = (horizon + 120_000.0).max(180_000.0);
        state.desired = state
            .catalog
            .keys()
            .filter_map(|&key| {
                let distance = eye.distance(anchor(key));
                if distance > range {
                    return None;
                }
                let around = (eye - anchor(key)).abs() - DVec3::splat(PATCH_SIZE * 0.5 + 12_000.0);
                let near = state
                    .active
                    .get(&key)
                    .is_some_and(|entry| entry.0 == Detail::Near);
                let threshold = NEAR_RANGE + if near { LOD_HYSTERESIS } else { 0.0 };
                let detail = if around.max(DVec3::ZERO).length() < threshold {
                    Detail::Near
                } else {
                    Detail::Far
                };
                Some((key, detail))
            })
            .collect();
        let state = &mut *state;
        state.active.retain(|key, (_, entity, mesh)| {
            if state.desired.contains_key(key) {
                return true;
            }
            commands.entity(*entity).despawn();
            meshes.remove(mesh.id());
            false
        });

        let mut queue: Vec<_> = state
            .desired
            .iter()
            .filter(|&(key, detail)| {
                state
                    .pending
                    .get(key)
                    .is_none_or(|pending| &pending.0 != detail)
                    && state.active.get(key).is_none_or(|entry| &entry.0 != detail)
            })
            .map(|(&key, &detail)| (key, detail))
            .collect();
        queue.sort_unstable_by(|a, b| {
            eye.distance_squared(anchor(a.0))
                .total_cmp(&eye.distance_squared(anchor(b.0)))
        });
        state.queue = queue.into();
    }

    let state = &mut *state;
    let mut uploads = 0;
    state.pending.retain(|&key, (detail, task)| {
        if state.desired.get(&key) != Some(detail) {
            return false;
        }
        if uploads >= MAX_UPLOADS {
            return true;
        }
        let Some(mesh) = block_on(poll_once(task)) else {
            return true;
        };
        let mesh = meshes.add(mesh);
        let (cell, translation) = frame.translation_to_grid(anchor(key));
        let entity = root
            .spawn(
                &mut commands,
                (
                    cell,
                    Transform::from_translation(translation),
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.0.clone()),
                    NotShadowCaster,
                ),
            )
            .id();
        if let Some((_, old, mesh)) = state.active.insert(key, (*detail, entity, mesh)) {
            commands.entity(old).despawn();
            meshes.remove(mesh.id());
        }
        uploads += 1;
        false
    });

    let budget = MAX_JOBS.saturating_sub(state.pending.len());
    for _ in 0..budget {
        let Some((key, detail)) = state.queue.pop_front() else {
            break;
        };
        let data = state.catalog[&key].clone();
        let heights = heights.clone();
        let task = AsyncComputeTaskPool::get()
            .spawn(async move { build_patch(&data, anchor(key), &heights, detail) });
        state.pending.insert(key, (detail, task));
    }
}

#[derive(Default)]
pub(super) struct WaterMesh {
    positions: Vec<[f32; 3]>,
    directions: Vec<[f32; 3]>,
    parameters: Vec<[f32; 2]>,
    optical: Vec<[f32; 4]>,
    pub(super) indices: Vec<u32>,
}

impl WaterMesh {
    pub(super) fn vertex(
        &mut self,
        anchor: DVec3,
        up: DVec3,
        elevation: f64,
        depth: f64,
        current: DVec3,
        parameters: [f32; 2],
    ) -> u32 {
        let index = self.positions.len() as u32;
        self.positions
            .push((up * (RADIUS + elevation) - anchor).as_vec3().to_array());
        self.directions.push(up.as_vec3().to_array());
        self.parameters.push(parameters);
        self.optical.push(
            current
                .as_vec3()
                .extend(depth.clamp(-20.0, 96.0) as f32)
                .to_array(),
        );
        index
    }

    fn grid(
        &mut self,
        columns: u32,
        rows: u32,
        invert: bool,
        mut vertex: impl FnMut(&mut Self, f64, f64),
    ) {
        let first = self.positions.len() as u32;
        let stride = columns + 1;
        for row in 0..=rows {
            for col in 0..=columns {
                vertex(
                    self,
                    f64::from(col) / f64::from(columns),
                    f64::from(row) / f64::from(rows),
                );
                if row > 0 && col > 0 {
                    let d = first + row * stride + col;
                    let (a, b, c) = (d - stride - 1, d - stride, d - 1);
                    let outward = if invert {
                        [a, c, b, b, c, d]
                    } else {
                        [a, b, c, b, d, c]
                    };
                    self.indices.extend(outward);
                }
            }
        }
    }

    pub(super) fn finish(self) -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, self.directions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.parameters);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, self.optical);
        mesh.insert_indices(Indices::U32(self.indices));
        mesh
    }
}

fn build_patch(patch: &Patch, anchor: DVec3, heights: &SurfaceHeights, detail: Detail) -> Mesh {
    let mut mesh = WaterMesh::default();
    for &tile in &patch.lakes {
        lake(
            &mut mesh,
            heights.lake_tiles()[tile],
            anchor,
            heights,
            detail,
        );
    }
    for &reach in &patch.rivers {
        river(
            &mut mesh,
            heights.river_reaches()[reach],
            anchor,
            heights,
            detail,
        );
    }
    mesh.finish()
}

fn lake(
    mesh: &mut WaterMesh,
    tile: LakeTile,
    anchor: DVec3,
    heights: &SurfaceHeights,
    detail: Detail,
) {
    let [a, b, c, d] = tile.corners;
    let invert = (b - a).cross(d - a).dot(a) < 0.0;
    let segments = if detail == Detail::Near { 10 } else { 2 };
    mesh.grid(segments, segments, invert, |mesh, x, y| {
        let up = a.lerp(b, x).lerp(d.lerp(c, x), y).normalize();
        let elevation = tile.elevation;
        let ground = if detail == Detail::Near {
            heights.sample(up)
        } else {
            heights.sample_base(up)
        };
        mesh.vertex(
            anchor,
            up,
            elevation,
            elevation - ground.unwrap_or(elevation - 4.0),
            DVec3::ZERO,
            [0.0, 1.0],
        );
    });
}

fn river(
    mesh: &mut WaterMesh,
    reach: RiverReach,
    anchor: DVec3,
    heights: &SurfaceHeights,
    detail: Detail,
) {
    let near = detail == Detail::Near;
    let along = (reach.length() / if near { 230.0 } else { 1700.0 }).ceil() as u32;
    let across = if near { 12 } else { 4 };
    let vertex = |mesh: &mut WaterMesh, up, elevation, bank: f64| {
        let ground = heights.sample(up).unwrap_or(elevation - 1.0);
        mesh.vertex(
            anchor,
            up,
            elevation,
            elevation - ground,
            reach.current(up),
            [bank as f32, 0.0],
        )
    };
    // Include a transparent toe beyond the nominal bank.
    mesh.grid(across, along.max(1), true, |mesh, x, t| {
        let cross = 2.0 * x - 1.0;
        let bank = cross.signum() * (1.0 - (1.0 - cross.abs()).powf(1.65)) * 1.04;
        vertex(mesh, reach.direction(t, bank), reach.elevation(t), bank);
    });
    // Cap the upstream side of each junction so angled tributaries join the
    // graded main stem. The downstream half is already covered by this strip.
    let up = reach.direction(0.0, 0.0);
    let stage = reach.elevation(0.0);
    let center = vertex(mesh, up, stage, 0.0);
    let width = reach.half_width(0.0) * 1.04 / RADIUS;
    let across = reach.across();
    let upstream = -across.cross(up);
    let segments = if near { 16 } else { 8 };
    for i in 0..=segments {
        let angle = f64::from(i) * std::f64::consts::PI / f64::from(segments);
        let (sin, cos) = angle.sin_cos();
        let edge = (up + (across * cos + upstream * sin) * width).normalize();
        let index = vertex(mesh, edge, stage, 1.04);
        if i != 0 {
            mesh.indices.extend([center, index - 1, index]);
        }
    }
}

pub(super) fn draw_map(
    heights: Res<SurfaceHeights>,
    player: Query<&GlobalTransform, With<Explorer>>,
    camera: Query<(&GlobalTransform, &WorldCamera)>,
    frames: ReferenceFrames,
    root: Res<PlanetRoot>,
    mut gizmos: Gizmos<RiverMapLines>,
) {
    let (Ok(player), Ok((camera, mode))) = (player.single(), camera.single()) else {
        return;
    };
    if *mode != WorldCamera::Map {
        return;
    }
    let distance = camera.translation().distance(player.translation());
    let opacity = ((distance - 500.0) / 700.0).clamp(0.0, 1.0);
    let Some(frame) = frames.parent_frame(root.entity()).filter(|_| opacity > 0.0) else {
        return;
    };
    let floating = frame.local_floating_origin();
    let origin = frame.grid_position_double(
        &floating.cell(),
        &Transform::from_translation(floating.translation()),
    );
    let lift = (distance * 0.010).clamp(6.0, 285.0);
    let color = Color::srgba(0.24, 0.81, 0.98, opacity * 0.88);
    for reach in heights.river_reaches() {
        let end = |t| {
            (reach.direction(t, 0.0) * (RADIUS + reach.elevation(t) + f64::from(lift)) - origin)
                .as_vec3()
        };
        gizmos.line(end(0.0), end(1.0), color);
    }
}

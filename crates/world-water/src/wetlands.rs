//! Streams shallow pools using the same height provider as the terrain mesh.

use bevy::{
    asset::RenderAssetUsages,
    light::NotShadowCaster,
    math::DVec3,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
};
use world_core::{
    planet::RADIUS,
    runtime::{GridTransform, PlanetRoot, ReferenceFrames, WorldCamera, WorldMovement},
    LocalPool, SurfaceHeights,
};

const RANGE: f64 = 240.0;
const REBUILD_DISTANCE: f64 = 65.0;
const ALTITUDE_LIMIT: f64 = 155.0;
const SEGMENTS: u16 = 48;

#[derive(Resource, Default)]
struct Wetlands {
    origin: Option<DVec3>,
    pending: Option<Task<Vec<(DVec3, Mesh)>>>,
    visible: Vec<(Entity, Handle<Mesh>)>,
    material: Handle<StandardMaterial>,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<Wetlands>()
        .add_systems(Update, stream.after(WorldMovement));
}

fn stream(
    mut commands: Commands,
    mut wetlands: ResMut<Wetlands>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    heights: Res<SurfaceHeights>,
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
    let up = eye.normalize_or_zero();
    let position = up * RADIUS;
    let ground = heights.sample(up).unwrap_or(0.0);
    if eye.length() - RADIUS - ground > ALTITUDE_LIMIT {
        wetlands.origin = None;
        wetlands.pending = None;
        retire(&mut wetlands, &mut commands, &mut meshes);
        return;
    }

    if wetlands
        .origin
        .is_none_or(|last| last.distance(position) > REBUILD_DISTANCE)
    {
        if wetlands
            .origin
            .is_some_and(|last| last.distance(position) > RANGE)
        {
            retire(&mut wetlands, &mut commands, &mut meshes);
        }
        let heights = heights.clone();
        wetlands.origin = Some(position);
        wetlands.pending = Some(AsyncComputeTaskPool::get().spawn(async move {
            heights
                .local_pools(up, RANGE)
                .into_iter()
                .map(|pool| {
                    let anchor = pool.center * (RADIUS + pool.elevation);
                    (anchor, pool_mesh(pool, anchor, &heights))
                })
                .collect()
        }));
    }

    let Some(batch) = wetlands
        .pending
        .as_mut()
        .and_then(|task| block_on(poll_once(task)))
    else {
        return;
    };
    wetlands.pending = None;
    retire(&mut wetlands, &mut commands, &mut meshes);
    if batch.is_empty() {
        return;
    }
    if wetlands.material == Handle::default() {
        wetlands.material = materials.add(StandardMaterial {
            base_color: Color::srgba(0.24, 0.39, 0.36, 0.72),
            perceptual_roughness: 0.19,
            reflectance: 0.65,
            alpha_mode: AlphaMode::Blend,
            ..default()
        });
    }
    let material = wetlands.material.clone();
    for (anchor, mesh) in batch {
        let (cell, translation) = frame.translation_to_grid(anchor);
        let mesh = meshes.add(mesh);
        let entity = root
            .spawn(
                &mut commands,
                (
                    cell,
                    Transform::from_translation(translation),
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    NotShadowCaster,
                ),
            )
            .id();
        wetlands.visible.push((entity, mesh));
    }
}

fn retire(wetlands: &mut Wetlands, commands: &mut Commands, meshes: &mut Assets<Mesh>) {
    for (entity, mesh) in wetlands.visible.drain(..) {
        commands.entity(entity).despawn();
        meshes.remove(&mesh);
    }
}

fn pool_mesh(pool: LocalPool, anchor: DVec3, heights: &SurfaceHeights) -> Mesh {
    let east = pool.center.any_orthonormal_vector();
    let north = pool.center.cross(east);
    let rings = (pool.radius / 1.6).ceil() as u16;
    let mut positions = Vec::with_capacity(usize::from(1 + rings * SEGMENTS));
    let mut colors = Vec::with_capacity(positions.capacity());
    let mut indices = Vec::with_capacity(usize::from(2 * rings * SEGMENTS * 3));
    let mut vertex = |up: DVec3| {
        positions.push(
            (up * (RADIUS + pool.elevation) - anchor)
                .as_vec3()
                .to_array(),
        );
        let depth = pool.elevation - heights.sample(up).unwrap_or(pool.elevation);
        colors.push([1.0, 1.0, 1.0, (depth / 0.18).clamp(0.0, 1.0) as f32]);
    };
    vertex(pool.center);
    for ring in 0..rings {
        let fraction = f64::from(ring + 1) / f64::from(rings);
        let start = 1 + ring * SEGMENTS;
        for segment in 0..SEGMENTS {
            let angle = std::f64::consts::TAU * f64::from(segment) / f64::from(SEGMENTS);
            let (sin, cos) = angle.sin_cos();
            let offset = (east * cos + north * sin) * (fraction * pool.radius / RADIUS);
            vertex((pool.center + offset).normalize());
            let a = start + segment;
            let b = start + (segment + 1) % SEGMENTS;
            if ring == 0 {
                indices.extend_from_slice(&[0, a, b]);
            } else {
                indices.extend_from_slice(&[a - SEGMENTS, a, b, a - SEGMENTS, b, b - SEGMENTS]);
            }
        }
    }
    let normals = vec![pool.center.as_vec3().to_array(); positions.len()];
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U16(indices));
    mesh
}

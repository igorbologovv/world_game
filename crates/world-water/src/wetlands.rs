//! Streams shallow pools using the same height provider as the terrain mesh.

use bevy::{
    light::NotShadowCaster,
    math::DVec3,
    prelude::*,
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
};
use world_core::{
    planet::RADIUS,
    runtime::{GridTransform, PlanetRoot, ReferenceFrames, WorldCamera, WorldMovement},
    LocalPool, SurfaceHeights,
};

use super::river_surface::{InlandMaterial, WaterMesh};

const RANGE: f64 = 240.0;
const REBUILD_DISTANCE: f64 = 65.0;
const ALTITUDE_LIMIT: f64 = 155.0;
const SEGMENTS: u16 = 48;

#[derive(Resource, Default)]
struct Wetlands {
    origin: Option<DVec3>,
    pending: Option<Task<Vec<(DVec3, Mesh)>>>,
    visible: Vec<(Entity, Handle<Mesh>)>,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<Wetlands>()
        .add_systems(Update, stream.after(WorldMovement));
}

fn stream(
    mut commands: Commands,
    mut wetlands: ResMut<Wetlands>,
    mut meshes: ResMut<Assets<Mesh>>,
    material: Res<InlandMaterial>,
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
                    let point = pool.center * (RADIUS + pool.elevation);
                    let anchor = (point / 128.0).round() * 128.0;
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
                    MeshMaterial3d(material.0.clone()),
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
    let mut mesh = WaterMesh::default();
    let vertex = |mesh: &mut WaterMesh, up: DVec3| {
        let depth = pool.elevation - heights.sample(up).unwrap_or(pool.elevation);
        mesh.vertex(anchor, up, pool.elevation, depth, DVec3::ZERO, [0.0, 2.0]);
    };
    vertex(&mut mesh, pool.center);
    for ring in 0..rings {
        let fraction = f64::from(ring + 1) / f64::from(rings);
        let start = 1 + ring * SEGMENTS;
        for segment in 0..SEGMENTS {
            let angle = std::f64::consts::TAU * f64::from(segment) / f64::from(SEGMENTS);
            let (sin, cos) = angle.sin_cos();
            let offset = (east * cos + north * sin) * (fraction * pool.radius / RADIUS);
            vertex(&mut mesh, (pool.center + offset).normalize());
            let a = u32::from(start + segment);
            let b = u32::from(start + (segment + 1) % SEGMENTS);
            if ring == 0 {
                mesh.indices.extend_from_slice(&[0, a, b]);
            } else {
                mesh.indices.extend_from_slice(&[
                    a - u32::from(SEGMENTS),
                    a,
                    b,
                    a - u32::from(SEGMENTS),
                    b,
                    b - u32::from(SEGMENTS),
                ]);
            }
        }
    }
    mesh.finish()
}

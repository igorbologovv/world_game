use std::{collections::HashMap, fs, path::Path};

use bevy::{
    asset::RenderAssetUsages,
    gizmos::config::GizmoConfigGroup,
    material::AlphaMode,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    reflect::TypePath,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};

use world_core::{
    coordinates::terrain_aligned_direction,
    planet::{
        BAKED_WATER_MESH_HEADER, LAKE_SURFACE_PATH, RIVER_REACH_VERTICES, RIVER_SURFACE_PATH,
    },
    planet::{LEGACY_BAKED_RADIUS, RADIUS},
    runtime::{Explorer, WorldCamera},
    SurfaceHeights,
};

#[derive(Component)]
pub(super) struct RiverWater;

// Bevy's view globals supply animation time directly; no per-frame CPU updates.
#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub(super) struct RiverMaterial {
    #[uniform(0)]
    pub(super) brightness: f32,
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

#[derive(Default, Reflect, GizmoConfigGroup)]
pub(super) struct RiverMapLines;

#[derive(Resource)]
pub(super) struct RiverCenterlines(Vec<[Vec3; 2]>);

pub(super) fn load(assets: &Path, heights: &SurfaceHeights) -> Option<(Mesh, RiverCenterlines)> {
    let positions = load_positions(&assets.join(RIVER_SURFACE_PATH), RIVER_REACH_VERTICES)?;
    Some(river_mesh(positions, heights))
}

/// Use flat lake tiles with a zero flow vector and no river-map centerlines.
pub(super) fn load_lakes(assets: &Path, heights: &SurfaceHeights) -> Option<Mesh> {
    let positions = load_positions(&assets.join(LAKE_SURFACE_PATH), 6)?;
    // Adjacent lake triangles share exactly the same baked positions. Index
    // them before evaluating the expensive depth and staging GPU attributes.
    let mut lookup = HashMap::new();
    let mut vertices = Vec::new();
    let mut depths = Vec::new();
    let mut indices = Vec::with_capacity(positions.len());
    for position in positions {
        let index = *lookup.entry(position.map(f32::to_bits)).or_insert_with(|| {
            let index = vertices.len() as u32;
            vertices.push(position);
            depths.push([
                0.0,
                0.0,
                0.0,
                water_depth(Vec3::from_array(position), heights),
            ]);
            index
        });
        indices.push(index);
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.5, 0.5]; vertices.len()]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, depths);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vertices);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

fn load_positions(path: &Path, record_vertices: usize) -> Option<Vec<[f32; 3]>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!(
                "Could not load inland water surface {}: {error}",
                path.display()
            );
            return None;
        }
    };
    let (bytes, baked) = match bytes.strip_prefix(BAKED_WATER_MESH_HEADER) {
        Some(vertices) => (vertices, true),
        None => (bytes.as_slice(), false),
    };
    if bytes.is_empty() {
        return None;
    }
    if bytes.len() % (12 * record_vertices) != 0 {
        warn!(
            "Invalid inland water surface geometry in {}",
            path.display()
        );
        return None;
    }
    let mut positions = Vec::with_capacity(bytes.len() / 12);
    for vertex in bytes.chunks_exact(12) {
        let coordinate =
            |offset| f32::from_le_bytes(vertex[offset..offset + 4].try_into().unwrap());
        let position = Vec3::new(coordinate(0), coordinate(4), coordinate(8));
        if !position.is_finite() || position.length_squared() == 0.0 {
            warn!("Invalid inland water surface vertex in {}", path.display());
            return None;
        }
        let aligned = if baked {
            position
        } else {
            let position = position.as_dvec3();
            (terrain_aligned_direction(position.normalize()) * position.length()).as_vec3()
        };
        // Existing water binaries were generated for the 100 km reference
        // sphere. Until hydrology is regenerated, preserve their radial
        // elevations while moving them onto the Mars-sized reference body.
        // Modern assets already near RADIUS are left untouched.
        let previous_radius = f64::from(aligned.length());
        let aligned = if (previous_radius - LEGACY_BAKED_RADIUS).abs() < 30_000.0 {
            (aligned.as_dvec3().normalize() * (RADIUS + previous_radius - LEGACY_BAKED_RADIUS))
                .as_vec3()
        } else {
            aligned
        };
        positions.push(aligned.to_array());
    }
    Some(positions)
}

// Height below the water surface, sampled from the actual generated terrain.
// Fall back to a modest depth only if the height source is unavailable.
fn water_depth(position: Vec3, heights: &SurfaceHeights) -> f32 {
    let radial = position.as_dvec3();
    let Some(ground) = heights.sample(radial.normalize()) else {
        return 8.0;
    };
    (radial.length() - RADIUS - ground).clamp(0.0, 96.0) as f32
}

fn river_mesh(positions: Vec<[f32; 3]>, heights: &SurfaceHeights) -> (Mesh, RiverCenterlines) {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    let centerlines: Vec<[Vec3; 2]> = positions
        .chunks_exact(RIVER_REACH_VERTICES)
        .map(|reach| {
            let start = Vec3::from_array(reach[6]);
            let left = Vec3::from_array(reach[1]);
            let right = Vec3::from_array(reach[5]);
            let end = (left + right).normalize() * ((left.length() + right.length()) * 0.5);
            [start, end]
        })
        .collect();
    // The strip tapers with the channel. Its vertices specify the banks exactly,
    // while the round cap at each junction uses the local bank direction.
    let mut uvs = Vec::with_capacity(positions.len());
    let mut flow_velocities = Vec::with_capacity(positions.len());
    let strip_banks = [0.0, 0.0, 1.0, 1.0, 0.0, 1.0];
    for (reach, &[start, end]) in positions
        .chunks_exact(RIVER_REACH_VERTICES)
        .zip(centerlines.iter())
    {
        let flow = (end - start).normalize_or_zero();
        let left_start = Vec3::from_array(reach[0]);
        let right_start = Vec3::from_array(reach[2]);
        let bank_vector = right_start - left_start;
        let width = bank_vector.length().max(0.01);
        let across = bank_vector / width;
        let slope = ((start.length() - end.length()) / start.distance(end).max(1.0)).max(0.0);
        let velocity = flow * (0.75 + 9.0 * slope.sqrt()).min(4.5);
        for (index, vertex) in reach.iter().enumerate() {
            let offset = Vec3::from_array(*vertex) - start;
            let bank = strip_banks
                .get(index)
                .copied()
                .unwrap_or_else(|| 0.5 + offset.dot(across) / width);
            uvs.push([offset.dot(flow), bank]);
            flow_velocities.push([
                velocity.x,
                velocity.y,
                velocity.z,
                water_depth(Vec3::from_array(*vertex), heights),
            ]);
        }
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, flow_velocities);
    (mesh, RiverCenterlines(centerlines))
}

pub(super) fn draw_map(
    rivers: Option<Res<RiverCenterlines>>,
    water: Query<&GlobalTransform, With<RiverWater>>,
    player: Query<&GlobalTransform, With<Explorer>>,
    camera: Query<(&GlobalTransform, &WorldCamera)>,
    mut gizmos: Gizmos<RiverMapLines>,
) {
    let (Some(rivers), Ok(water), Ok(player), Ok((camera, mode))) =
        (rivers, water.single(), player.single(), camera.single())
    else {
        return;
    };
    if *mode != WorldCamera::Map {
        return;
    }
    let distance = camera.translation().distance(player.translation());
    let opacity = ((distance - 500.0) / 700.0).clamp(0.0, 1.0);
    if opacity == 0.0 {
        return;
    }
    let lift = (distance * 0.010).clamp(6.0, 285.0);
    let color = Color::srgba(0.24, 0.81, 0.98, opacity * 0.88);
    let transform = water.affine();
    for &[start, end] in &rivers.0 {
        let start = transform.transform_point3(start + start.normalize() * lift);
        let end = transform.transform_point3(end + end.normalize() * lift);
        gizmos.line(start, end, color);
    }
}

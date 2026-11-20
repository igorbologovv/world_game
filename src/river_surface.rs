use std::fs;

use bevy::{
    gizmos::config::GizmoConfigGroup,
    prelude::*,
    render::{render_asset::RenderAssetUsages, render_resource::PrimitiveTopology},
};

use bevy_terrain::prelude::DebugCameraController;

use crate::{
    planet::{RIVER_REACH_VERTICES, RIVER_SURFACE_PATH},
    player::Player,
    surface_heights::{asset_directory, terrain_aligned_direction},
};

#[derive(Component)]
pub struct RiverWater;

// Bevy's view globals supply animation time directly; no per-frame CPU updates.
#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub struct RiverMaterial {
    #[uniform(0)]
    pub brightness: f32,
}

impl Material for RiverMaterial {
    fn fragment_shader() -> bevy::render::render_resource::ShaderRef {
        "shaders/river_water.wgsl".into()
    }
}

#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct RiverMapLines;

#[derive(Resource)]
pub struct RiverCenterlines(Vec<[Vec3; 2]>);

pub fn load() -> Option<(Mesh, RiverCenterlines)> {
    let path = asset_directory().join(RIVER_SURFACE_PATH);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!("Could not load river surface {}: {error}", path.display());
            return None;
        }
    };
    if bytes.is_empty() || bytes.len() % (12 * RIVER_REACH_VERTICES) != 0 {
        warn!("Invalid river surface geometry in {}", path.display());
        return None;
    }
    let mut positions = Vec::with_capacity(bytes.len() / 12);
    let mut normals = Vec::with_capacity(bytes.len() / 12);
    for vertex in bytes.chunks_exact(12) {
        let coordinate =
            |offset| f32::from_le_bytes(vertex[offset..offset + 4].try_into().unwrap());
        let position = Vec3::new(coordinate(0), coordinate(4), coordinate(8));
        if !position.is_finite() || position.length_squared() == 0.0 {
            warn!("Invalid river surface vertex in {}", path.display());
            return None;
        }
        let position = position.as_dvec3();
        let normal = terrain_aligned_direction(position.normalize());
        positions.push((normal * position.length()).as_vec3().to_array());
        normals.push(normal.as_vec3().to_array());
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    let centerlines = positions
        .chunks_exact(RIVER_REACH_VERTICES)
        .map(|reach| {
            let start = Vec3::from_array(reach[6]);
            let left = Vec3::from_array(reach[1]);
            let right = Vec3::from_array(reach[5]);
            let end = (left + right).normalize() * ((left.length() + right.length()) * 0.5);
            [start, end]
        })
        .collect();
    // Assign each triangle downstream distance (U, in meters) and bank-to-bank
    // offset (V). Generate UVs from the existing surface: no river binary format
    // changes and no regeneration of world assets are required.
    let mut uvs = Vec::with_capacity(positions.len());
    for reach in positions.chunks_exact(RIVER_REACH_VERTICES) {
        let start = Vec3::from_array(reach[6]);
        let left_end = Vec3::from_array(reach[1]);
        let right_end = Vec3::from_array(reach[5]);
        let end = (left_end + right_end).normalize()
            * ((left_end.length() + right_end.length()) * 0.5);
        let flow = (end - start).normalize_or_zero();
        let left_start = Vec3::from_array(reach[0]);
        let right_start = Vec3::from_array(reach[2]);
        let bank_vector = right_start - left_start;
        let width = bank_vector.length().max(0.01);
        let across = bank_vector / width;
        for vertex in reach {
            let offset = Vec3::from_array(*vertex) - start;
            uvs.push([offset.dot(flow), 0.5 + offset.dot(across) / width]);
        }
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    Some((mesh, RiverCenterlines(centerlines)))
}

pub fn draw_map(
    rivers: Option<Res<RiverCenterlines>>,
    water: Query<&GlobalTransform, With<RiverWater>>,
    player: Query<&GlobalTransform, With<Player>>,
    camera: Query<&GlobalTransform, With<DebugCameraController>>,
    mut gizmos: Gizmos<RiverMapLines>,
) {
    let (Some(rivers), Ok(water), Ok(player), Ok(camera)) = (
        rivers,
        water.get_single(),
        player.get_single(),
        camera.get_single(),
    ) else {
        return;
    };
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

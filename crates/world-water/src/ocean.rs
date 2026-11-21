use std::path::Path;

use bevy::{
    math::{DVec2, DVec3},
    prelude::*,
    reflect::TypePath,
    render::{
        mesh::Indices,
        render_asset::RenderAssetUsages,
        render_resource::{
            AsBindGroup, Extent3d, PrimitiveTopology, ShaderRef, TextureDimension, TextureFormat,
        },
        texture::ImageSampler,
    },
};
use world_core::{
    coordinates::{cube_direction, height_source_uv},
    planet::{decode_height, OCEAN_ATLAS_PATH, RADIUS, WATER_SURFACE_ELEVATION},
    runtime::{GridTransform, ReferenceFrames, WorldCamera},
    SurfaceHeights,
};

const WATER_RADIUS: f64 = RADIUS + WATER_SURFACE_ELEVATION;
const PATCHES_PER_FACE: u32 = 8;
// 384 intervals across a face keep the mesh within one meter of the water sphere.
const SEGMENTS: u32 = 48;
// The previous sRGB texture classified a pixel as water at 0.25 linear.
const MIN_WATER: u8 = 137;
const DEPTH_OFFSET: f64 = 16.0;
const DEPTH_RANGE: f64 = 96.0;

#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub(super) struct OceanMaterial {
    #[texture(0)]
    #[sampler(1)]
    mask: Handle<Image>,
}

impl Material for OceanMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/ocean.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/ocean.wgsl".into()
    }
}

#[derive(Component)]
pub(super) struct OceanPatch {
    direction: DVec3,
    angle_cos: f64,
    angle_sin: f64,
}

pub(super) fn load(
    images: &mut Assets<Image>,
    heights: &SurfaceHeights,
    assets: &Path,
) -> Option<(OceanMaterial, Vec<(Mesh, OceanPatch)>)> {
    let path = assets.join(OCEAN_ATLAS_PATH);
    let mask = match image::open(&path) {
        Ok(mask) => mask.into_luma8(),
        Err(error) => {
            warn!("Could not load water mask {}: {error}", path.display());
            return None;
        }
    };
    let side = mask.width() / 3;
    if side == 0 || mask.width() != side * 3 || mask.height() != side * 2 {
        warn!("Invalid water mask dimensions in {}", path.display());
        return None;
    }

    let mut patches = Vec::new();
    for face in 0..6 {
        let origin = UVec2::new(face as u32 % 3, face as u32 / 3) * side;
        for y in 0..PATCHES_PER_FACE {
            for x in 0..PATCHES_PER_FACE {
                let start = DVec2::new(f64::from(x), f64::from(y)) / f64::from(PATCHES_PER_FACE);
                let end = start + 1.0 / f64::from(PATCHES_PER_FACE);
                let first = source_pixel(start, side).floor().as_uvec2() + origin;
                let last = source_pixel(end, side).ceil().as_uvec2() + origin;
                let has_water = (first.y..=last.y).any(|y| {
                    let row = (y * mask.width()) as usize;
                    mask.as_raw()[row + first.x as usize..=row + last.x as usize]
                        .iter()
                        .any(|&value| value >= MIN_WATER)
                });
                if has_water {
                    patches.push(build_patch(face, start, end, origin, side));
                }
            }
        }
    }
    let mut pixels = vec![0; mask.as_raw().len() * 2];
    for face in 0..6 {
        let origin = UVec2::new(face as u32 % 3, face as u32 / 3) * side;
        let texels = heights.face_texels(face, side);
        for y in 0..side {
            let atlas_start = ((origin.y + y) * mask.width() + origin.x) as usize;
            let source_start = (y * side) as usize;
            for x in 0..side as usize {
                let atlas_index = atlas_start + x;
                let coverage = mask.as_raw()[atlas_index];
                let depth = texels.map_or(45.0, |values| {
                    WATER_RADIUS - RADIUS - decode_height(f64::from(values[source_start + x]))
                });
                // Keep signed depth around the waterline for bilinear shore sampling.
                pixels[2 * atlas_index] = coverage;
                pixels[2 * atlas_index + 1] = encode_depth(depth);
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: mask.width(),
            height: mask.height(),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rg8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    Some((
        OceanMaterial {
            mask: images.add(image),
        },
        patches,
    ))
}

fn encode_depth(depth: f64) -> u8 {
    ((depth + DEPTH_OFFSET).clamp(0.0, DEPTH_RANGE) * (255.0 / DEPTH_RANGE)).round() as u8
}

fn source_pixel(uv: DVec2, side: u32) -> DVec2 {
    height_source_uv(uv, DVec2::splat(f64::from(side))) * f64::from(side - 1)
}

fn build_patch(
    face: usize,
    start: DVec2,
    end: DVec2,
    origin: UVec2,
    side: u32,
) -> (Mesh, OceanPatch) {
    let stride = (SEGMENTS + 1) as usize;
    let mut positions = Vec::with_capacity(stride * stride);
    let mut uvs = Vec::with_capacity(stride * stride);
    let mut indices = Vec::with_capacity((SEGMENTS * SEGMENTS * 6) as usize);
    let mid = cube_direction(face, (start + end) * 0.5);
    let mut angular_radius: f64 = 0.0;
    let step = (end - start) / f64::from(SEGMENTS);

    for y in 0..=SEGMENTS {
        for x in 0..=SEGMENTS {
            let uv = start + step * DVec2::new(f64::from(x), f64::from(y));
            let direction = cube_direction(face, uv);
            angular_radius = angular_radius.max(mid.distance_squared(direction));
            positions.push((direction * WATER_RADIUS).as_vec3().to_array());
            uvs.push((origin.as_vec2() + source_pixel(uv, side).as_vec2()).to_array());
        }
    }

    for y in 0..SEGMENTS {
        for x in 0..SEGMENTS {
            let a = (y * (SEGMENTS + 1) + x) as u16;
            let b = a + 1;
            let c = a + stride as u16;
            let d = c + 1;
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U16(indices));
    // Pad the cap to cover the difference between triangle normals and the
    // radial directions at their vertices.
    let angle = 2.0 * (angular_radius.sqrt() * 0.5).asin() + 0.01;
    let (angle_sin, angle_cos) = angle.sin_cos();
    (
        mesh,
        OceanPatch {
            direction: mid,
            angle_cos,
            angle_sin,
        },
    )
}

pub(super) fn cull_patches(
    cameras: Query<(Entity, GridTransform), With<WorldCamera>>,
    frames: ReferenceFrames,
    mut patches: Query<(&OceanPatch, &mut Visibility)>,
) {
    let Ok((entity, camera)) = cameras.get_single() else {
        return;
    };
    let Some(frame) = frames.parent_frame(entity) else {
        return;
    };
    let eye = frame.grid_position_double(&camera.cell, &camera.transform);
    let eye_radius = eye.length();
    let eye_direction = eye.normalize_or_zero();
    let horizon_cos = ((WATER_RADIUS - 1.0) / eye_radius).min(1.0);
    let horizon_sin = (1.0 - horizon_cos * horizon_cos).sqrt();
    for (patch, mut visibility) in &mut patches {
        let limit = horizon_cos * patch.angle_cos - horizon_sin * patch.angle_sin;
        let visible = eye_direction.dot(patch.direction) >= limit;
        let desired = if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        visibility.set_if_neq(desired);
    }
}

#[cfg(test)]
mod tests {
    use bevy::render::mesh::VertexAttributeValues;

    use super::*;

    #[test]
    fn patch_triangles_face_outward_and_stay_above_sea_level() {
        // The corrected cube projection stretches vertices most near corners.
        let patch_size = 1.0 / f64::from(PATCHES_PER_FACE);
        for face in 0..6 {
            let (mesh, _) = build_patch(
                face,
                DVec2::ZERO,
                DVec2::splat(patch_size),
                UVec2::ZERO,
                1024,
            );
            let Some(VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("Expected ocean vertex positions");
            };
            let Some(Indices::U16(indices)) = mesh.indices() else {
                panic!("Expected indexed ocean triangles");
            };
            for triangle in indices.chunks_exact(3) {
                let point = |index| Vec3::from_array(positions[usize::from(index)]).as_dvec3();
                let a = point(triangle[0]);
                let b = point(triangle[1]);
                let c = point(triangle[2]);
                let normal = (b - a).cross(c - a).normalize();
                assert!(normal.dot(a) > RADIUS);
            }
        }
    }
}

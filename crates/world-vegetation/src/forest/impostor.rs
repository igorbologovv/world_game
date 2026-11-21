//! Portraits of the existing procedural meshes, packed into one alpha-tested mipmapped atlas.
use bevy::{prelude::*, render::render_resource::TextureFormat};

use super::{
    generate_tree, proxy::half_width, textures, tree_height, MeshBuilder, Species, FOLIAGE_CUTOFF,
    VARIANTS,
};

const SIDE: usize = 128;
pub(super) const TILES: usize = Species::ALL.len() * VARIANTS;
const INSET: f32 = 0.022;

pub(super) fn uv(species: Species, variant: usize, uv: Vec2) -> Vec2 {
    let tile = (species as usize * VARIANTS + variant) as f32;
    Vec2::new(
        (tile + INSET + uv.x * (1.0 - 2.0 * INSET)) / TILES as f32,
        INSET + uv.y * (1.0 - 2.0 * INSET),
    )
}

pub(super) fn bake(oak: Option<&Image>) -> Image {
    let width = SIDE * TILES;
    let mut pixels = vec![[0; 4]; width * SIDE];
    for species in Species::ALL {
        let diffuse = textures::canopy_texture(species);
        for variant in 0..VARIANTS {
            let portrait = oak
                .filter(|_| species == Species::Oak)
                .and_then(exported_oak)
                .unwrap_or_else(|| render_portrait(species, variant, &diffuse));
            let tile = species as usize * VARIANTS + variant;
            for (y, row) in portrait.chunks_exact(SIDE).enumerate() {
                let start = y * width + tile * SIDE;
                pixels[start..start + SIDE].copy_from_slice(row);
            }
        }
    }
    textures::mipmapped(pixels, width, SIDE, TILES, Some(0.46), false)
}

fn exported_oak(image: &Image) -> Option<Vec<[u8; 4]>> {
    if image.texture_descriptor.format != TextureFormat::Rgba8UnormSrgb {
        return None;
    }
    let mut pixels = Vec::with_capacity(SIDE * SIDE);
    for y in 0..SIDE {
        for x in 0..SIDE {
            pixels.push(sample(
                image,
                Vec2::new(x as f32, y as f32) / (SIDE - 1) as f32,
            ));
        }
    }
    Some(pixels)
}

fn render_portrait(species: Species, variant: usize, diffuse: &Image) -> Vec<[u8; 4]> {
    let (trunk, canopy) = generate_tree(species, variant as u64, 1);
    let height = tree_height(species, variant as u64);
    let area = Vec2::new(half_width(species) * 1.16 * 2.0, 1.07) * height;
    let rotation = Quat::from_rotation_y(0.38 + variant as f32 * 0.71);
    let mut portrait = Portrait {
        color: vec![[0; 4]; SIDE * SIDE],
        depth: vec![f32::NEG_INFINITY; SIDE * SIDE],
    };
    portrait.draw(trunk, rotation, area, None);
    portrait.draw(canopy, rotation, area, Some(diffuse));
    portrait.color
}

struct Portrait {
    color: Vec<[u8; 4]>,
    depth: Vec<f32>,
}

impl Portrait {
    fn draw(&mut self, mesh: MeshBuilder, rotation: Quat, area: Vec2, diffuse: Option<&Image>) {
        let vertices: Vec<_> = mesh
            .positions
            .iter()
            .map(|&raw| {
                let point = rotation * Vec3::from_array(raw);
                let uv = Vec2::new(0.5 + point.x / area.x, 1.0 - point.y / area.y);
                (uv * (SIDE - 1) as f32, point.z)
            })
            .collect();
        for triangle in mesh.indices.chunks_exact(3) {
            let indices: [usize; 3] = std::array::from_fn(|i| triangle[i] as usize);
            let [a, b, c] = indices.map(|i| vertices[i].0);
            let determinant = (b - a).perp_dot(c - a);
            if determinant.abs() < 0.001 {
                continue;
            }
            let min = a
                .min(b)
                .min(c)
                .floor()
                .clamp(Vec2::ZERO, Vec2::splat((SIDE - 1) as f32))
                .as_uvec2();
            let max = a
                .max(b)
                .max(c)
                .ceil()
                .clamp(Vec2::ZERO, Vec2::splat((SIDE - 1) as f32))
                .as_uvec2();
            for y in min.y..=max.y {
                for x in min.x..=max.x {
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - a;
                    let v = p.perp_dot(c - a) / determinant;
                    let w = (b - a).perp_dot(p) / determinant;
                    let weights = [1.0 - v - w, v, w];
                    if weights.into_iter().any(|weight| weight < 0.0) {
                        continue;
                    }
                    let index = y as usize * SIDE + x as usize;
                    let depth = blend(indices.map(|i| vertices[i].1), weights);
                    if depth < self.depth[index] {
                        continue;
                    }
                    let uv = blend(indices.map(|i| Vec2::from_array(mesh.uvs[i])), weights);
                    let texel = diffuse.map_or([255; 4], |image| sample(image, uv));
                    if f32::from(texel[3]) < FOLIAGE_CUTOFF * 255.0 {
                        continue;
                    }
                    let color = blend(
                        indices.map(|i| Vec4::from_array(mesh.colors[i]).truncate()),
                        weights,
                    );
                    let normal = rotation
                        * blend(indices.map(|i| Vec3::from_array(mesh.normals[i])), weights)
                            .normalize_or_zero();
                    let shade = 0.76 + 0.38 * normal.dot(Vec3::new(-0.4, 0.72, 0.55)).abs();
                    let rgb = color
                        * Vec3::new(
                            (f32::from(texel[0]) / 255.0).powi(2),
                            (f32::from(texel[1]) / 255.0).powi(2),
                            (f32::from(texel[2]) / 255.0).powi(2),
                        )
                        * shade;
                    self.color[index] = [
                        (rgb.x.clamp(0.0, 1.0).sqrt() * 255.0) as u8,
                        (rgb.y.clamp(0.0, 1.0).sqrt() * 255.0) as u8,
                        (rgb.z.clamp(0.0, 1.0).sqrt() * 255.0) as u8,
                        255,
                    ];
                    self.depth[index] = depth;
                }
            }
        }
    }
}

fn blend<T: std::ops::Mul<f32, Output = T> + std::ops::Add<Output = T>>(
    values: [T; 3],
    weights: [f32; 3],
) -> T {
    let [a, b, c] = values;
    a * weights[0] + b * weights[1] + c * weights[2]
}

fn sample(image: &Image, uv: Vec2) -> [u8; 4] {
    let size = image.texture_descriptor.size;
    let x = (uv.x.clamp(0.0, 1.0) * (size.width - 1) as f32).round() as usize;
    let y = (uv.y.clamp(0.0, 1.0) * (size.height - 1) as f32).round() as usize;
    let i = (y * size.width as usize + x) * 4;
    image.data.as_ref().expect("CPU image bytes")[i..i + 4].try_into().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_portrait_has_a_mipmapped_cutout() {
        let image = bake(None);
        assert!(image.texture_descriptor.mip_level_count > 3);
        let width = SIDE * TILES;
        for tile in 0..TILES {
            let alpha = (0..SIDE)
                .flat_map(|y| (0..SIDE).map(move |x| (y * width + tile * SIDE + x) * 4 + 3));
            let solid = alpha
                .clone()
                .filter(|&index| image.data.as_ref().expect("CPU image bytes")[index] >= 120)
                .count();
            assert!(
                (50..SIDE * SIDE / 2).contains(&solid),
                "portrait {tile} has {solid} solid texels"
            );
        }
    }
}

//! Procedural and authored tree portraits in one mipmapped atlas.
use std::sync::Arc;

use bevy::{mesh::VertexAttributeValues, prelude::*, render::render_resource::TextureFormat};

use super::{
    generate_tree, proxy::half_width, textures, tree_height, MeshBuilder, Species, FOLIAGE_CUTOFF,
    VARIANTS,
};

// Keep canopy silhouettes crisp while moving from the near mesh to distant
// proxy cards; a 128px portrait is clearly pixelated at the near handoff.
const SIDE: usize = 256;
const TILES: usize = Species::Palm as usize * VARIANTS + Species::Palm.variants();
const INSET: f32 = 0.022;

fn tile(species: Species, variant: usize) -> usize {
    species as usize * VARIANTS + variant
}

#[derive(Clone, Default)]
pub(super) struct Portraits {
    tiles: [Option<Arc<[[u8; 4]]>>; TILES],
    pub revision: usize,
}

impl Portraits {
    pub fn insert(&mut self, species: Species, variant: usize, pixels: Vec<[u8; 4]>) {
        self.tiles[tile(species, variant)] = Some(pixels.into());
        self.revision += 1;
    }
}

pub(super) fn uv(species: Species, variant: usize, uv: Vec2) -> Vec2 {
    let tile = tile(species, variant) as f32;
    Vec2::new(
        (tile + INSET + uv.x * (1.0 - 2.0 * INSET)) / TILES as f32,
        INSET + uv.y * (1.0 - 2.0 * INSET),
    )
}

pub(super) fn bake(oak: Option<&Image>, authored: &Portraits) -> Image {
    let width = SIDE * TILES;
    let mut pixels = vec![[0; 4]; width * SIDE];
    for species in Species::ALL {
        let diffuse = textures::canopy_texture(species);
        for variant in 0..species.variants() {
            let tile = tile(species, variant);
            let fallback;
            let portrait = if let Some(pixels) = &authored.tiles[tile] {
                pixels.as_ref()
            } else {
                fallback = oak
                    .filter(|_| species == Species::Oak && variant == 0)
                    .and_then(exported_oak)
                    .unwrap_or_else(|| render_portrait(species, variant, &diffuse));
                &fallback
            };
            for (y, row) in portrait.chunks_exact(SIDE).enumerate() {
                let start = y * width + tile * SIDE;
                pixels[start..start + SIDE].copy_from_slice(row);
            }
        }
    }
    textures::mipmapped(pixels, width, SIDE, TILES, Some(0.46), false)
}

fn centered_inset(uv: Vec2) -> Vec2 {
    Vec2::splat(INSET) + uv * (1.0 - 2.0 * INSET)
}

fn exported_oak(image: &Image) -> Option<Vec<[u8; 4]>> {
    if image.texture_descriptor.format != TextureFormat::Rgba8UnormSrgb {
        return None;
    }
    let mut pixels = Vec::with_capacity(SIDE * SIDE);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let uv =
                (Vec2::new(x as f32, y as f32) / (SIDE - 1) as f32 - INSET) / (1.0 - 2.0 * INSET);
            pixels.push(if uv.min_element() >= 0.0 && uv.max_element() <= 1.0 {
                sample(image, uv, false)
            } else {
                [0; 4]
            });
        }
    }
    Some(pixels)
}

fn render_portrait(species: Species, variant: usize, diffuse: &Image) -> Vec<[u8; 4]> {
    let (trunk, canopy) = generate_tree(species, variant as u64, 1);
    let mut portrait = Portrait::new(species, variant);
    portrait.draw(&trunk, None, 0.0, false);
    portrait.draw(&canopy, Some(diffuse), FOLIAGE_CUTOFF, false);
    portrait.color
}

pub(super) fn render_authored(
    species: Species,
    variant: usize,
    parts: [(&Mesh, &Image, f32, bool); 2],
) -> Option<Vec<[u8; 4]>> {
    let mut portrait = Portrait::new(species, variant);
    for (mesh, diffuse, cutoff, repeat) in parts {
        if diffuse.texture_descriptor.format != TextureFormat::Rgba8UnormSrgb {
            return None;
        }
        let VertexAttributeValues::Float32x3(positions) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
        else {
            return None;
        };
        let VertexAttributeValues::Float32x3(normals) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL)?
        else {
            return None;
        };
        let VertexAttributeValues::Float32x2(uvs) = mesh.attribute(Mesh::ATTRIBUTE_UV_0)? else {
            return None;
        };
        let geometry = MeshBuilder {
            positions: positions.clone(),
            normals: normals.clone(),
            uvs: uvs.clone(),
            colors: vec![[1.0; 4]; positions.len()],
            indices: mesh.indices()?.iter().map(|index| index as u32).collect(),
            ..default()
        };
        portrait.draw(&geometry, Some(diffuse), cutoff, repeat);
    }
    Some(portrait.color)
}

struct Portrait {
    color: Vec<[u8; 4]>,
    depth: Vec<f32>,
    area: Vec2,
    rotation: Quat,
}

impl Portrait {
    fn new(species: Species, variant: usize) -> Self {
        let height = tree_height(species, variant as u64);
        Self {
            color: vec![[0; 4]; SIDE * SIDE],
            depth: vec![f32::NEG_INFINITY; SIDE * SIDE],
            area: Vec2::new(half_width(species, variant) * 1.16 * 2.0, 1.07) * height,
            rotation: Quat::from_rotation_y(0.38 + variant as f32 * 0.71),
        }
    }

    fn draw(&mut self, mesh: &MeshBuilder, diffuse: Option<&Image>, cutoff: f32, repeat: bool) {
        let vertices: Vec<_> = mesh
            .positions
            .iter()
            .map(|&raw| {
                let point = self.rotation * Vec3::from_array(raw);
                let uv = Vec2::new(0.5 + point.x / self.area.x, 1.0 - point.y / self.area.y);
                (centered_inset(uv) * (SIDE - 1) as f32, point.z)
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
                    let texel = diffuse.map_or([255; 4], |image| sample(image, uv, repeat));
                    if f32::from(texel[3]) < cutoff * 255.0 {
                        continue;
                    }
                    let color = blend(
                        indices.map(|i| Vec4::from_array(mesh.colors[i]).truncate()),
                        weights,
                    );
                    let normal = self.rotation
                        * blend(indices.map(|i| Vec3::from_array(mesh.normals[i])), weights)
                            .normalize_or_zero();
                    let shade = 0.88 + 0.23 * normal.dot(Vec3::new(-0.4, 0.72, 0.55)).abs();
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

fn sample(image: &Image, uv: Vec2, repeat: bool) -> [u8; 4] {
    let uv = if repeat { uv - uv.floor() } else { uv };
    let size = image.texture_descriptor.size;
    let x = (uv.x.clamp(0.0, 1.0) * (size.width - 1) as f32).round() as usize;
    let y = (uv.y.clamp(0.0, 1.0) * (size.height - 1) as f32).round() as usize;
    let i = (y * size.width as usize + x) * 4;
    image.data.as_ref().expect("CPU image bytes")[i..i + 4]
        .try_into()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_portrait_has_a_mipmapped_cutout() {
        let image = bake(None, &Portraits::default());
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

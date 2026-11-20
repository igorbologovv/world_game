use std::{env, path::PathBuf};

use bevy::{
    math::{DVec2, DVec3},
    prelude::*,
};

use crate::{
    planet::HEIGHT_MAP_SIZE, LOD_COUNT, MAX_HEIGHT, MIN_HEIGHT, PATH, TERRAIN_BORDER_SIZE,
    TEXTURE_SIZE,
};

const CUBE_CORRECTION: f64 = 0.87 * 0.87;
const FINE_TILE_COUNT: u32 = 1 << (LOD_COUNT - 1);
const TEXEL_CENTER_OFFSET: f64 =
    0.5 / ((TEXTURE_SIZE - 2 * TERRAIN_BORDER_SIZE) * FINE_TILE_COUNT) as f64;
type HeightFace = image::ImageBuffer<image::Luma<u16>, Vec<u16>>;

#[derive(Resource)]
pub struct SurfaceHeights {
    faces: Option<[HeightFace; 6]>,
    rivers: Option<image::GrayImage>,
}

impl SurfaceHeights {
    pub fn load() -> Self {
        let assets = asset_directory();
        let source = assets.join(PATH).join("source/height");
        let faces: Option<Vec<_>> = (0..6)
            .map(|face| {
                image::open(source.join(format!("face{face}.tif")))
                    .ok()
                    .map(|image| image.into_luma16())
            })
            .collect();
        let rivers = image::open(assets.join("world/river_atlas.png"))
            .ok()
            .and_then(|image| {
                let image = image.into_rgba8();
                let side = image.width() / 3;
                if side == 0 || image.width() != side * 3 || image.height() != side * 2 {
                    return None;
                }
                let (width, height) = image.dimensions();
                let coverage = image
                    .into_raw()
                    .chunks_exact(4)
                    .map(|pixel| pixel[3])
                    .collect();
                image::GrayImage::from_raw(width, height, coverage)
            });
        Self {
            faces: faces.and_then(|faces| faces.try_into().ok()),
            rivers,
        }
    }

    pub fn sample(&self, direction: DVec3) -> Option<f64> {
        let (face, uv) = surface_uv(direction);
        let image = &self.faces.as_ref()?[face];
        let dimensions = DVec2::new(f64::from(image.width()), f64::from(image.height()));
        let uv = height_source_uv(uv, dimensions);
        let height = bilinear(uv, image.width(), image.height(), |x, y| {
            f64::from(image.get_pixel(x, y).0[0])
        });
        Some(
            f64::from(MIN_HEIGHT)
                + height / f64::from(u16::MAX) * f64::from(MAX_HEIGHT - MIN_HEIGHT),
        )
    }

    pub fn is_river(&self, direction: DVec3) -> bool {
        let Some(map) = &self.rivers else {
            return false;
        };
        let (face, uv) = surface_uv(direction);
        let uv = height_source_uv(uv, DVec2::splat(f64::from(HEIGHT_MAP_SIZE)));
        let side = map.width() / 3;
        let x_offset = (face as u32 % 3) * side;
        let y_offset = (face as u32 / 3) * side;
        let coverage = bilinear(uv, side, side, |x, y| {
            f64::from(map.get_pixel(x + x_offset, y + y_offset).0[0])
        });
        coverage >= 156.0
    }
}

pub fn asset_directory() -> PathBuf {
    env::var_os("BEVY_ASSET_ROOT")
        .or_else(|| env::var_os("CARGO_MANIFEST_DIR"))
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("assets")
}

pub fn terrain_aligned_direction(direction: DVec3) -> DVec3 {
    let (face, source) = surface_uv(direction);
    let size = f64::from(HEIGHT_MAP_SIZE);
    let baked = (source * (size - 1.0) + 0.5) / size + TEXEL_CENTER_OFFSET;
    let uv = baked.clamp(DVec2::ZERO, DVec2::ONE) * 2.0 - 1.0;
    let plane = uv / (1.0 + CUBE_CORRECTION - CUBE_CORRECTION * uv * uv).powf(0.5);
    let DVec2 { x: u, y: v } = plane;
    match face {
        0 => DVec3::new(-1.0, -v, u),
        1 => DVec3::new(u, -v, 1.0),
        2 => DVec3::new(u, 1.0, v),
        3 => DVec3::new(1.0, -u, v),
        4 => DVec3::new(v, -u, -1.0),
        _ => DVec3::new(v, -1.0, u),
    }
    .normalize()
}

fn height_source_uv(uv: DVec2, dimensions: DVec2) -> DVec2 {
    let pixel = ((uv - TEXEL_CENTER_OFFSET) * dimensions - 0.5).max(DVec2::ZERO);
    let last = (dimensions - 1.0).max(DVec2::ONE);
    (pixel / last).min(DVec2::ONE)
}

fn surface_uv(direction: DVec3) -> (usize, DVec2) {
    let absolute = direction.abs();
    let DVec3 { x, y, z } = direction;
    let (face, plane) = if absolute.x > absolute.y && absolute.x > absolute.z {
        if x < 0.0 {
            (0, DVec2::new(z, -y) / -x)
        } else {
            (3, DVec2::new(-y, z) / x)
        }
    } else if absolute.z > absolute.y {
        if z > 0.0 {
            (1, DVec2::new(x, -y) / z)
        } else {
            (4, DVec2::new(-y, x) / -z)
        }
    } else if y > 0.0 {
        (2, DVec2::new(x, z) / y)
    } else {
        (5, DVec2::new(z, x) / -y)
    };
    let corrected =
        plane * ((1.0 + CUBE_CORRECTION) / (1.0 + CUBE_CORRECTION * plane * plane)).powf(0.5);
    let uv = (corrected * 0.5 + 0.5).clamp(DVec2::ZERO, DVec2::ONE);
    (face, uv)
}

fn bilinear(uv: DVec2, width: u32, height: u32, sample: impl Fn(u32, u32) -> f64) -> f64 {
    let last_x = width - 1;
    let last_y = height - 1;
    let pixel = uv * DVec2::new(f64::from(last_x), f64::from(last_y));
    let x = pixel.x.floor() as u32;
    let y = pixel.y.floor() as u32;
    let right = (x + 1).min(last_x);
    let down = (y + 1).min(last_y);
    let top = sample(x, y) + (sample(right, y) - sample(x, y)) * pixel.x.fract();
    let bottom = sample(x, down) + (sample(right, down) - sample(x, down)) * pixel.x.fract();
    top + (bottom - top) * pixel.y.fract()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn river_and_baked_height_uvs_agree_on_all_cube_faces() {
        let directions = [
            DVec3::new(-1.0, 0.3, 0.4),
            DVec3::new(0.35, -0.2, 1.0),
            DVec3::new(-0.4, 1.0, 0.55),
            DVec3::new(1.0, -0.6, 0.35),
            DVec3::new(0.2, -0.4, -1.0),
            DVec3::new(-0.3, -1.0, -0.55),
        ];
        for direction in directions {
            let (face, source) = surface_uv(direction);
            let (aligned_face, aligned) = surface_uv(terrain_aligned_direction(direction));
            assert_eq!(aligned_face, face);
            let decoded = height_source_uv(aligned, DVec2::splat(f64::from(HEIGHT_MAP_SIZE)));
            assert!(decoded.abs_diff_eq(source, 1e-12));
        }
    }
}

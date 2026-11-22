use std::{path::Path, sync::Arc};

use bevy_ecs::prelude::Resource;
use bevy_math::{DVec2, DVec3};

use crate::{
    coordinates::{height_source_uv, surface_uv},
    planet::{decode_height, HEIGHT_MAP_SIZE, HEIGHT_SOURCE_PATH, RIVER_ATLAS_PATH},
    terrain_detail,
};

type HeightFace = image::ImageBuffer<image::Luma<u16>, Vec<u16>>;

/// Read-only height and river coverage queries shared by simulation and rendering.
///
/// Clones share the immutable raster data with streaming workers.
#[derive(Clone, Resource)]
pub struct SurfaceHeights {
    faces: Option<Arc<[HeightFace; 6]>>,
    rivers: Option<Arc<image::GrayImage>>,
}

impl SurfaceHeights {
    /// Read legacy TIFF height faces and the river coverage atlas from an asset directory.
    /// Missing height faces or river coverage leave the corresponding queries unavailable.
    pub fn load(assets: &Path) -> Self {
        let source = assets.join(HEIGHT_SOURCE_PATH);
        let faces: Option<Vec<_>> = (0..6)
            .map(|face| {
                image::open(source.join(format!("face{face}.tif")))
                    .ok()
                    .map(|image| image.into_luma16())
            })
            .collect();
        let rivers = image::open(assets.join(RIVER_ATLAS_PATH))
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
            faces: faces.and_then(|faces| faces.try_into().ok()).map(Arc::new),
            rivers: rivers.map(Arc::new),
        }
    }

    /// Sample elevation in meters above the reference sphere along a radial direction.
    pub fn sample(&self, direction: DVec3) -> Option<f64> {
        let (face, uv) = face_uv(direction)?;
        let base = sample_height(&self.faces.as_ref()?[face], uv);
        if base <= 1.5 || self.faces.as_ref()?[0].width() != HEIGHT_MAP_SIZE {
            return Some(base);
        }
        Some((base + terrain_detail::detail(direction, base, self.river_at(face, uv))).max(1.5))
    }

    /// Interpolate only the pre-baked global terrain, without procedural detail.
    /// Hydrology still uses these values until the regional water rebuild.
    pub fn sample_base(&self, direction: DVec3) -> Option<f64> {
        let (face, uv) = face_uv(direction)?;
        Some(sample_height(&self.faces.as_ref()?[face], uv))
    }

    /// Borrow a complete legacy height face if it matches the requested square resolution.
    pub fn face_texels(&self, face: usize, side: u32) -> Option<&[u16]> {
        let image = self.faces.as_ref()?.get(face)?;
        (image.dimensions() == (side, side)).then_some(image.as_raw().as_slice())
    }

    /// Bilinearly sampled legacy river mask, in 0..=255 coverage units.
    /// This can smoothly suppress additional terrain detail near baked channels.
    pub fn river_coverage(&self, direction: DVec3) -> f64 {
        face_uv(direction).map_or(0.0, |(face, uv)| self.river_at(face, uv))
    }

    fn river_at(&self, face: usize, uv: DVec2) -> f64 {
        let Some(map) = &self.rivers else {
            return 0.0;
        };
        let uv = height_source_uv(uv, DVec2::splat(f64::from(HEIGHT_MAP_SIZE)));
        let side = map.width() / 3;
        let x_offset = (face as u32 % 3) * side;
        let y_offset = (face as u32 / 3) * side;
        bilinear(uv, side, side, |x, y| {
            f64::from(map.get_pixel(x + x_offset, y + y_offset).0[0])
        })
    }

    /// Query river coverage at a terrain-aligned radial direction.
    pub fn is_river(&self, direction: DVec3) -> bool {
        self.river_coverage(direction) >= 156.0
    }
}

fn face_uv(direction: DVec3) -> Option<(usize, DVec2)> {
    (direction != DVec3::ZERO && direction.is_finite()).then(|| surface_uv(direction))
}

fn sample_height(image: &HeightFace, uv: DVec2) -> f64 {
    let dimensions = DVec2::new(f64::from(image.width()), f64::from(image.height()));
    let uv = height_source_uv(uv, dimensions);
    decode_height(bilinear(uv, image.width(), image.height(), |x, y| {
        f64::from(image.get_pixel(x, y).0[0])
    }))
}

fn bilinear(uv: DVec2, width: u32, height: u32, sample: impl Fn(u32, u32) -> f64) -> f64 {
    let last_x = width - 1;
    let last_y = height - 1;
    let pixel = uv * DVec2::new(f64::from(last_x), f64::from(last_y));
    let x = pixel.x.floor() as u32;
    let y = pixel.y.floor() as u32;
    let right = (x + 1).min(last_x);
    let down = (y + 1).min(last_y);
    let top_left = sample(x, y);
    let bottom_left = sample(x, down);
    let dx = pixel.x.fract();
    let top = top_left + (sample(right, y) - top_left) * dx;
    let bottom = bottom_left + (sample(right, down) - bottom_left) * dx;
    top + (bottom - top) * pixel.y.fract()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_and_river_queries_use_the_same_cube_face() {
        let mut rivers = image::GrayImage::new(6, 4);
        for y in 2..4 {
            for x in 0..2 {
                rivers.put_pixel(x, y, image::Luma([255]));
            }
        }
        let heights = SurfaceHeights {
            faces: Some(Arc::new(std::array::from_fn(|face| {
                HeightFace::from_pixel(2, 2, image::Luma([20_000 + face as u16]))
            }))),
            rivers: Some(Arc::new(rivers)),
        };

        assert_eq!(heights.sample(DVec3::X), Some(decode_height(20_003.0)));
        assert_eq!(heights.sample(DVec3::NEG_X), Some(decode_height(20_000.0)));
        assert_eq!(heights.sample(DVec3::ZERO), None);
        assert!(heights.is_river(DVec3::X));
        assert!(!heights.is_river(DVec3::NEG_X));
    }
}

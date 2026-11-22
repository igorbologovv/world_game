//! A prefiltered relief texture traced from the canonical trees in the finest layer.

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageSampler, ImageSamplerDescriptor},
    math::DVec2,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

use crate::forest::{placement::Site, proxy::half_width};

const SIDE: usize = 1024;
const MIP_TEXELS: usize = (SIDE * SIDE * 4 - 1) / 3;
const HEIGHT_RANGE: f32 = 40.0;

pub(super) fn image() -> Image {
    let mut result = Image::new_fill(
        Extent3d {
            width: SIDE as u32,
            height: SIDE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0; 4],
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::default(),
    );
    result.data = Some(vec![0; MIP_TEXELS * 4]);
    result.texture_descriptor.mip_level_count = SIDE.ilog2() + 1;
    let mut sampler = ImageSamplerDescriptor::linear();
    sampler.set_anisotropic_filter(8);
    result.sampler = ImageSampler::Descriptor(sampler);
    result
}

pub(super) struct CrownRaster {
    pixels: Vec<[u8; 4]>,
    pixels_per_meter: f32,
}

impl CrownRaster {
    pub(super) fn new(half_width: f64) -> Self {
        let mut pixels = Vec::with_capacity(MIP_TEXELS);
        pixels.resize(SIDE * SIDE, [0; 4]);
        Self {
            pixels,
            pixels_per_meter: SIDE as f32 / (2.0 * half_width as f32),
        }
    }

    pub(super) fn add(&mut self, site: Site, square: DVec2, orientation: Vec2) {
        let center = (square.as_vec2() * 0.5 + 0.5) * SIDE as f32 - 0.5;
        let height = site.height();
        let radius = half_width(site.species, site.variant) * height * self.pixels_per_meter;
        let reach = Vec2::splat(radius * 1.28 + 0.65);
        let lo = (center - reach).ceil().as_ivec2().max(IVec2::ZERO);
        let hi = (center + reach)
            .floor()
            .as_ivec2()
            .min(IVec2::splat(SIDE as i32 - 1));
        let rotation = Vec2::new(orientation.x, -orientation.y);
        // Yaw and scale come from the same per-tree seed as placement, and
        // remain stable when the camera-centered atlas is regenerated.
        let leaf_tone = (site.yaw * 2.73).sin() * 0.32 + 0.5;
        let leaf = site.species.colors().1;
        let warmth = (0.48 + (leaf[0] - leaf[2]) * 1.7 + (site.scale - 1.0) * 0.53).clamp(0.0, 1.0);
        let aspect = 0.75 + 0.22 * leaf_tone;
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let offset = (Vec2::new(x as f32, y as f32) - center) / radius;
                let mut point = rotation.rotate(offset);
                point.y /= aspect;
                let distance = point.length();
                let radial = point / distance.max(0.002);
                // Harmonics generate rounded lobes and prevent circular, uniformly spaced blobs.
                let cos3 = radial.x * (4.0 * radial.x * radial.x - 3.0);
                let y2 = radial.y * radial.y;
                let sin5 = radial.y * ((16.0 * y2 - 20.0) * y2 + 5.0);
                let outline = 1.0 + 0.17 * cos3 + 0.095 * sin5;
                let alpha = (0.5 + (outline - distance) * radius).clamp(0.0, 1.0);
                if alpha < 0.008 {
                    continue;
                }
                let dome = (1.0 - (distance / outline).powi(2)).max(0.0).sqrt();
                let folds = (0.038 * cos3 + 0.026 * sin5) * distance.min(1.0);
                let top = (height / HEIGHT_RANGE * (0.59 + 0.41 * dome + folds)).min(1.0);
                let target = &mut self.pixels[y as usize * SIDE + x as usize];
                let occupied_height = quantize(top * alpha);
                if occupied_height > target[1] {
                    *target = [
                        quantize(alpha),
                        occupied_height,
                        quantize(leaf_tone * alpha),
                        quantize(warmth * alpha),
                    ];
                }
            }
        }
    }

    pub(super) fn finish(self) -> Vec<u8> {
        mip_chain(self.pixels, SIDE).into_iter().flatten().collect()
    }
}

fn quantize(value: f32) -> u8 {
    (value * 255.0).round() as u8
}

fn mip_chain(mut pixels: Vec<[u8; 4]>, mut side: usize) -> Vec<[u8; 4]> {
    let mut offset = 0;
    while side > 1 {
        for y in (0..side).step_by(2) {
            for x in (0..side).step_by(2) {
                let start = offset + y * side + x;
                let taps = [
                    pixels[start],
                    pixels[start + 1],
                    pixels[start + side],
                    pixels[start + side + 1],
                ];
                // Height and color were premultiplied during rasterization,
                // so mipmaps average occupied crowns and gaps independently.
                pixels.push(std::array::from_fn(|channel| {
                    ((taps
                        .iter()
                        .map(|pixel| u16::from(pixel[channel]))
                        .sum::<u16>()
                        + 2)
                        / 4) as u8
                }));
            }
        }
        offset += side * side;
        side /= 2;
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forest::Species;
    use bevy::math::DVec3;

    #[test]
    fn filtering_keeps_premultiplied_crown_height() {
        let levels = mip_chain(vec![[0; 4], [160, 80, 40, 120], [0; 4], [0; 4]], 2);
        assert_eq!(levels[4], [40, 20, 10, 30]);
    }

    #[test]
    fn a_crown_has_varied_relief_and_a_non_circular_footprint() {
        let mut raster = CrownRaster::new(512.0);
        raster.add(
            Site {
                position: DVec3::Y,
                species: Species::Jungle,
                variant: 1,
                scale: 1.0,
                yaw: 0.0,
            },
            DVec2::ZERO,
            Vec2::X,
        );
        let pixel = |x, y| raster.pixels[(SIDE / 2 + y) * SIDE + SIDE / 2 + x];
        assert_eq!(pixel(0, 0)[0], 255);
        assert!(pixel(0, 0)[1] > pixel(8, 0)[1]);
        assert!(pixel(17, 0)[0] > pixel(0, 17)[0]);
        assert_eq!(pixel(32, 0), [0; 4]);
    }
}

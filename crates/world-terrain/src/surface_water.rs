//! Moisture near existing water surfaces, sampled while terrain meshes are built.

use std::{fs, path::Path, sync::Arc};

use bevy::{
    asset::RenderAssetUsages,
    image::{CompressedImageFormats, ImageType},
    math::{DVec2, DVec3},
    prelude::*,
    render::render_resource::TextureFormat,
};
use world_core::{
    coordinates::{height_source_uv, source_face_uv, surface_uv},
    planet::{LAKE_MASK_PATH, RADIUS, WATER_SURFACE_ELEVATION},
    LocalPool, SurfaceHeights,
};

#[derive(Clone, Default)]
pub(super) struct LakeMasks(Option<Arc<[Option<LakeFace>; 6]>>);

struct LakeFace {
    size: UVec2,
    coverage: Vec<u8>,
}

impl LakeMasks {
    pub(super) fn load(assets: &Path) -> Self {
        let directory = assets.join(LAKE_MASK_PATH);
        Self(Some(Arc::new(std::array::from_fn(|face| {
            let bytes = fs::read(directory.join(format!("face{face}.png"))).ok()?;
            let image = Image::from_buffer(
                &bytes,
                ImageType::Extension("png"),
                CompressedImageFormats::empty(),
                false,
                default(),
                RenderAssetUsages::MAIN_WORLD,
            )
            .ok()?;
            let size = image.size();
            let channels = match image.texture_descriptor.format {
                TextureFormat::Rgba8Unorm => 4,
                TextureFormat::R8Unorm => 1,
                _ => return None,
            };
            let coverage = image
                .data?
                .into_iter()
                .step_by(channels)
                .collect::<Vec<_>>();
            coverage
                .iter()
                .any(|&wet| wet != 0)
                .then_some(LakeFace { size, coverage })
        }))))
    }

    fn coverage(&self, direction: DVec3) -> f64 {
        let Some(faces) = &self.0 else { return 0.0 };
        let (face, uv) = surface_uv(direction);
        let at = |face: usize, uv| faces[face].as_ref().map_or(0.0, |mask| mask.sample(uv));
        let coverage = at(face, uv);
        const EDGE: f64 = 0.0025;
        if uv.min(DVec2::ONE - uv).min_element() >= EDGE {
            return coverage;
        }

        // The baked cube faces can disagree at an edge. Extend the wettest
        // neighbor's coverage across the seam with the same fade as river masks.
        let relative = direction.abs() / direction.abs().max_element();
        [
            (if direction.x < 0.0 { 0 } else { 3 }, relative.x),
            (if direction.y < 0.0 { 5 } else { 2 }, relative.y),
            (if direction.z < 0.0 { 4 } else { 1 }, relative.z),
        ]
        .into_iter()
        .filter(|&(neighbor, axis)| neighbor != face && axis > 1.0 - EDGE)
        .fold(coverage, |wet, (neighbor, axis)| {
            let uv = DVec2::from_array(source_face_uv(neighbor, direction.to_array()))
                .clamp(DVec2::ZERO, DVec2::ONE);
            wet.max(at(neighbor, uv) * smoothstep(1.0 - EDGE, 1.0, axis))
        })
    }
}

impl LakeFace {
    fn sample(&self, uv: DVec2) -> f64 {
        let last = self.size - UVec2::ONE;
        let pixel = height_source_uv(uv, self.size.as_dvec2()) * last.as_dvec2();
        let top_left = pixel.as_uvec2();
        let bottom_right = (top_left + UVec2::ONE).min(last);
        let at = |x, y| f64::from(self.coverage[(y * self.size.x + x) as usize]) / 255.0;
        let mix = |a, b, t| a + (b - a) * t;
        let top = mix(
            at(top_left.x, top_left.y),
            at(bottom_right.x, top_left.y),
            pixel.x.fract(),
        );
        let bottom = mix(
            at(top_left.x, bottom_right.y),
            at(bottom_right.x, bottom_right.y),
            pixel.x.fract(),
        );
        mix(top, bottom, pixel.y.fract())
    }
}

pub(super) fn influence(
    heights: &SurfaceHeights,
    lakes: &LakeMasks,
    pools: &[LocalPool],
    up: DVec3,
    height: f64,
) -> f32 {
    if height <= -9.0 {
        return 0.0;
    }
    let east = up.any_orthonormal_vector();
    let north = up.cross(east);
    let offsets = [east, north, -east, -north];
    let probe = |axis: DVec3, distance: f64| (up + axis * (distance / RADIUS)).normalize();

    let mut fresh = (heights.river_coverage(up) / 135.0).max(lakes.coverage(up));
    if fresh < 1.0 {
        for axis in offsets {
            // Use a softened bank mask on both sides of baked channels.
            let nearby = probe(axis, 34.0);
            let bank = (heights.river_coverage(nearby) / 195.0).max(lakes.coverage(nearby) * 0.80);
            fresh = fresh.max(bank);
        }
    }
    for pool in pools {
        let distance = up.distance(pool.center) * RADIUS;
        if distance < pool.radius + 36.0 {
            let lateral = 1.0 - smoothstep(pool.radius * 0.45, pool.radius + 36.0, distance);
            let vertical = 1.0 - smoothstep(0.15, 4.0, height - pool.elevation);
            fresh = fresh.max(lateral * vertical);
        }
    }

    let mut coast: f64 = 0.0;
    if height < 58.0 {
        for (distance, strength) in [(140.0, 1.0), (460.0, 0.65)] {
            for axis in offsets {
                if let Some(elevation) = heights.sample_base(probe(axis, distance)) {
                    coast = coast.max(
                        strength
                            * (1.0
                                - smoothstep(
                                    WATER_SURFACE_ELEVATION - 1.0,
                                    WATER_SURFACE_ELEVATION + 4.0,
                                    elevation,
                                )),
                    );
                }
            }
        }
        coast *= 1.0 - smoothstep(2.5, 58.0, height);
        coast = coast.max(1.0 - smoothstep(WATER_SURFACE_ELEVATION, 3.3, height));
    }
    // Positive values describe saltwater margins, negative freshwater margins.
    // Estuaries interpolate through the shared damp-soil palette.
    (coast - (1.0 - 0.35 * coast) * fresh.clamp(0.0, 1.0)) as f32
}

fn smoothstep(low: f64, high: f64, value: f64) -> f64 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use world_core::coordinates::{cube_direction, terrain_baked_uv};

    use super::*;

    #[test]
    fn shipped_lake_mask_maps_to_freshwater() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let lakes = LakeMasks::load(&assets);
        let faces = lakes.0.as_ref().unwrap();
        let (face, mask) = faces
            .iter()
            .enumerate()
            .find_map(|(face, mask)| mask.as_ref().map(|mask| (face, mask)))
            .expect("shipped planet has baked lakes");
        let offset = mask
            .coverage
            .iter()
            .position(|&value| value == 255)
            .unwrap() as u32;
        let texel = UVec2::new(offset % mask.size.x, offset / mask.size.x);
        let direction = cube_direction(
            face,
            terrain_baked_uv(texel.as_dvec2() / (mask.size - UVec2::ONE).as_dvec2()),
        );
        assert!(lakes.coverage(direction) > 0.98);
    }
}

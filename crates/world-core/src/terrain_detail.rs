//! Deterministic, world-space terrain detail layered over baked planetary heights.
//!
//! The six baked height faces encode continents and basins. Their resolution
//! must not limit close-range terrain: this function is continuous across
//! cube-face/tile boundaries and depends on *physical* coordinates, not tiles.
//! Keeping this in world-core makes collision, vegetation and rendering agree.

use bevy_math::{DVec3, IVec3, Vec3};

use crate::planet::RADIUS;

pub(crate) const BASIN_CELL: f64 = 112.0;

#[derive(Clone, Copy)]
pub(crate) struct Basin {
    pub(crate) center: DVec3,
    pub(crate) radius: f64,
    depth: f64,
}

impl Basin {
    pub(crate) fn at_cell(cell: IVec3) -> Option<Self> {
        let middle = (cell.as_dvec3() + DVec3::splat(0.5)) * BASIN_CELL;
        if (middle.length() - RADIUS).abs() > 28.0 {
            return None;
        }
        let hash = |seed| {
            lattice_hash(
                i64::from(cell.x),
                i64::from(cell.y),
                i64::from(cell.z),
                seed,
            )
        };
        if hash(0x83AD_D5EF) < -0.45 {
            return None;
        }
        let jitter = DVec3::new(hash(0x3B1C_A48F), hash(0xD47E_C102), hash(0x4084_6DE7)) * 5.0;
        let center = (middle + jitter).normalize();
        let radius = 17.0 + 5.0 * hash(0xBB15_71AB);
        // The depression must reach zero before the sampling cell ends.
        if (center * RADIUS - middle).abs().max_element() + radius >= BASIN_CELL * 0.5 {
            return None;
        }
        Some(Self {
            center,
            radius,
            depth: 3.1 + 0.8 * hash(0xD9A2_B7F3),
        })
    }

    fn relief(self, point: DVec3) -> f64 {
        let distance = point.distance(self.center * RADIUS) / self.radius;
        -self.depth * (1.0 - smoothstep(0.08, 1.0, distance)).powi(2)
    }
}

/// Scale-aware relief layered onto the planet's low-resolution global heightmap.
///
/// Continents and water basins come from the baked atlas, but highlands,
/// mountain ridges, foothills, valleys and small terrain features are
/// reconstructed continuously in physical meters (not from the atlas pixels).
/// The evaluated height is identical on both sides of a terrain chunk seam.
pub(crate) fn detail(
    direction: DVec3,
    base_height: f64,
    river_coverage: f64,
    lake_coverage: f64,
) -> f64 {
    let river = 1.0 - smoothstep(22.0, 200.0, river_coverage);
    if base_height <= 1.5 || river == 0.0 {
        return 0.0;
    }
    let up = direction.normalize();
    let point = up * RADIUS;
    // Entire frequency bands are absent at the coast and above their fade
    // ranges. Skip their hashes while keeping the same weights and samples.
    let inland = smoothstep(30.0, 580.0, base_height);
    let broad = if inland > 0.0 {
        let mountains = smoothstep(-0.16, 0.30, value_noise(point / 260_000.0, 0x9B5D_2F31));
        let ridge = if mountains > 0.0 {
            let ridgeline = 1.0 - value_noise(point / 58_000.0, 0xB83A_FD07).abs();
            ridgeline.powi(3) * mountains * 3_900.0
        } else {
            0.0
        };
        let valleys = value_noise(point / 91_000.0, 0x5BD1_8A19) * 1_180.0;
        let foothills = value_noise(point / 21_000.0, 0xA316_D13B) * 440.0;
        let rolling = value_noise(point / 5_000.0, 0xFE21_03AB) * 110.0;
        let slopes = value_noise(point / 1_200.0, 0x22A1_A42B) * 31.0;
        let ground = value_noise(point / 260.0, 0x5E80_5E91) * 7.5;
        let micro = value_noise(point / 48.0, 0x24AB_920C) * 1.2;
        (ridge + valleys + foothills + rolling + slopes + ground + micro) * inland * river
    } else {
        0.0
    };
    let highland = smoothstep(55.0, 240.0, base_height);
    let local = if highland > 0.0 {
        let region = smoothstep(-0.32, 0.13, value_noise(point / 94_000.0, 0x47D9_A3B1));
        if region > 0.0 {
            let spur = (1.0 - value_noise(point / 9_000.0, 0xF812_9C54).abs()).powi(3);
            let crags = (1.0 - value_noise(point / 2_800.0, 0x33F0_5E6A).abs()).powi(4);
            let ravines = (1.0 - value_noise(point / 1_050.0, 0xC78D_2A19).abs()).powi(3);
            region * (spur * 1_420.0 + crags * 360.0 + ravines * 75.0) * highland * river
        } else {
            0.0
        }
    } else {
        0.0
    };
    let dune_envelope =
        smoothstep(2.0, 25.0, base_height) * (1.0 - smoothstep(45.0, 200.0, base_height));
    let dunes = if dune_envelope > 0.0 {
        dune_envelope
            * (value_noise(point / 800.0, 0x7ABC_0317) * 18.0
                + value_noise(point / 110.0, 0xF0B4_219C) * 2.2)
    } else {
        0.0
    };
    let coastal_envelope =
        smoothstep(1.5, 10.0, base_height) * (1.0 - smoothstep(130.0, 350.0, base_height));
    let coastal_hills = if coastal_envelope > 0.0 {
        coastal_envelope
            * (value_noise(point / 1_300.0, 0x28EA_4B13).max(0.0) * 125.0
                + value_noise(point / 360.0, 0xA13D_72C0).max(0.0) * 22.0)
    } else {
        0.0
    };
    let legacy = broad + local + (coastal_hills + dunes) * river;
    let land =
        smoothstep(1.5, 13.0, base_height) * river * (1.0 - smoothstep(0.0, 0.55, lake_coverage));
    if land > 0.0 {
        legacy + land * biome_relief(up, point, base_height + legacy, river_coverage)
    } else {
        legacy
    }
}

fn biome_relief(up: DVec3, point: DVec3, elevation: f64, river_coverage: f64) -> f64 {
    let (temperature, moisture) = climate(up, elevation);
    let arid = smoothstep(0.65, 0.86, temperature) * (1.0 - smoothstep(0.19, 0.43, moisture));
    let exposed =
        smoothstep(850.0, 2_750.0, elevation).max(1.0 - smoothstep(0.18, 0.46, temperature));
    let marsh = wetland_weight(temperature, moisture, elevation, river_coverage, point);
    let hills = value_noise(point / 175.0, 0xB1E3_C218);
    let hummocks = value_noise(point / 31.0, 0x4618_7A27);
    let bumps = value_noise(point / 8.5, 0xA61B_65D9);
    let mut relief = hills * (13.0 - 8.5 * marsh - 5.5 * arid)
        + hummocks * (2.6 - 1.7 * marsh - 1.6 * arid)
        + bumps * (0.43 - 0.25 * marsh - 0.23 * arid);

    let erosion = (1.0 - marsh) * (1.0 - arid) * smoothstep(0.26, 0.68, moisture);
    if erosion > 0.0 {
        // The zero contours form branching hollows; a wider field limits the
        // cuts to valleys so they break up instead of circling every hill.
        let channel = value_noise(point / 83.0, 0xAD1C_3F67).abs();
        relief -= 5.5
            * erosion
            * (1.0 - smoothstep(0.06, 0.29, channel))
            * (1.0 - smoothstep(-0.18, 0.30, hills));
    }
    let rocky = (1.0 - marsh) * (1.0 - arid) * (0.26 + 0.74 * exposed);
    let rock = value_noise(point / 43.0, 0xCF18_9AC3);
    relief += 6.5 * rocky * smoothstep(0.29, 0.76, rock) * (0.8 + 0.5 * bumps);

    if arid > 0.0 {
        // Stretch the noise along a prevailing wind axis and vary the crest
        // position slowly so dune trains curve and break up.
        let crosswind = point.dot(DVec3::new(0.8, 0.0, 0.6));
        let along_wind = point.dot(DVec3::new(-0.6, 0.0, 0.8));
        let warp = value_noise(point / 220.0, 0x9DB4_6073);
        let bands = DVec3::new(
            along_wind / 55.0 + 0.8 * warp,
            point.y / 270.0,
            crosswind / 310.0,
        );
        let crest = value_noise(bands, 0x1858_D2F9);
        relief += arid * (8.5 * smoothstep(-0.45, 0.63, crest) + 0.35 * bumps);
    }
    if marsh > 0.0 {
        let pool = Basin::at_cell((point / BASIN_CELL).floor().as_ivec3());
        relief += marsh * pool.map_or(0.0, |basin| basin.relief(point));
    }
    relief
}

pub(crate) fn wetland_weight(
    temperature: f64,
    moisture: f64,
    elevation: f64,
    river_coverage: f64,
    point: DVec3,
) -> f64 {
    let river_moisture = smoothstep(0.0, 70.0, river_coverage) * 0.23;
    let lowland = 1.0 - smoothstep(110.0, 670.0, elevation);
    let wet = smoothstep(0.51, 0.74, moisture + river_moisture);
    if lowland == 0.0 || wet == 0.0 {
        return 0.0;
    }
    let pockets = 0.38 + 0.62 * smoothstep(-0.48, 0.45, value_noise(point / 620.0, 0x83E0_6AB1));
    lowland * wet * smoothstep(0.15, 0.34, temperature) * pockets
}

// Same climate fields as the terrain material and canopy placement. Use the
// macro elevation so hills and depressions do not move their own biome bands.
pub(crate) fn climate(up: DVec3, elevation: f64) -> (f64, f64) {
    let up = up.as_vec3();
    let weather = climate_noise(up * 5.8 + Vec3::new(17.3, -4.7, 11.8));
    let rain = climate_noise(up * 17.0 + Vec3::new(-2.1, 9.4, 4.6));
    let heat = climate_noise(up * 4.1 + Vec3::new(3.2, 18.6, -8.4));
    let moisture = (0.50 + 0.96 * weather + 0.32 * rain).clamp(0.0, 1.0);
    let temperature = (1.07 - 0.78 * f64::from(up.y.abs()) - elevation.max(0.0) / 16_000.0
        + 0.13 * f64::from(heat))
    .clamp(0.0, 1.0);
    (temperature, f64::from(moisture))
}

fn climate_noise(point: Vec3) -> f32 {
    let cell = point.floor();
    let fract = point - cell;
    let w = fract * fract * (Vec3::splat(3.0) - 2.0 * fract);
    let hash = |offset: Vec3| {
        let p = (cell + offset) * Vec3::new(0.1031, 0.1030, 0.0973);
        let mut h = p - p.floor();
        h += Vec3::splat(h.dot(Vec3::new(h.y, h.x, h.z) + Vec3::splat(33.33)));
        let v = (h.x + h.y) * h.z;
        v - v.floor()
    };
    let lerp = |a, b, t| a + (b - a) * t;
    let plane = |z| {
        lerp(
            lerp(hash(z), hash(z + Vec3::X), w.x),
            lerp(hash(z + Vec3::Y), hash(z + Vec3::X + Vec3::Y), w.x),
            w.y,
        )
    };
    lerp(plane(Vec3::ZERO), plane(Vec3::Z), w.z) - 0.5
}

fn smoothstep(start: f64, end: f64, x: f64) -> f64 {
    let t = ((x - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// Stable splitmix-style integer hashing: unlike randomized collection hashes,
// this has no process-dependent seed.
fn lattice_hash(x: i64, y: i64, z: i64, seed: u64) -> f64 {
    let mut v = seed
        ^ (x as u64).wrapping_mul(0x9E37_79B1_85EB_CA87)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (z as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    v ^= v >> 30;
    v = v.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    v ^= v >> 27;
    v = v.wrapping_mul(0x94D0_49BB_1331_11EB);
    v ^= v >> 31;
    (v as f64 / u64::MAX as f64) * 2.0 - 1.0
}

fn value_noise(p: DVec3, seed: u64) -> f64 {
    let cell = p.floor();
    let x = cell.x as i64;
    let y = cell.y as i64;
    let z = cell.z as i64;
    let f = p - cell;
    let t = f * f * (DVec3::splat(3.0) - 2.0 * f);
    // Keep the original summation order for deterministic heights. The
    // eight corners are fixed and don't need nested range iterators.
    let xy00 = (1.0 - t.x) * (1.0 - t.y);
    let xy10 = t.x * (1.0 - t.y);
    let xy01 = (1.0 - t.x) * t.y;
    let xy11 = t.x * t.y;
    let z0 = 1.0 - t.z;
    let mut sum = 0.0;
    sum += xy00 * z0 * lattice_hash(x, y, z, seed);
    sum += xy10 * z0 * lattice_hash(x + 1, y, z, seed);
    sum += xy01 * z0 * lattice_hash(x, y + 1, z, seed);
    sum += xy11 * z0 * lattice_hash(x + 1, y + 1, z, seed);
    sum += xy00 * t.z * lattice_hash(x, y, z + 1, seed);
    sum += xy10 * t.z * lattice_hash(x + 1, y, z + 1, seed);
    sum += xy01 * t.z * lattice_hash(x, y + 1, z + 1, seed);
    sum += xy11 * t.z * lattice_hash(x + 1, y + 1, z + 1, seed);
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_continuous_across_small_steps() {
        let d = DVec3::new(0.23, 0.95, -0.14).normalize();
        let a = detail(d, 1200.0, 0.0, 0.0);
        assert_eq!(a, detail(d, 1200.0, 0.0, 0.0));
        let east = d.cross(DVec3::Y).normalize();
        let b = detail((d + east * (0.01 / RADIUS)).normalize(), 1200.0, 0.0, 0.0);
        assert!((b - a).abs() < 0.15, "detail discontinuity: {a} vs {b}");
    }

    #[test]
    fn coastline_and_channels_are_undisturbed() {
        let d = DVec3::new(0.23, 0.95, -0.14).normalize();
        assert_eq!(detail(d, 0.0, 0.0, 0.0), 0.0);
        assert_eq!(detail(d, -1_500.0, 0.0, 0.0), 0.0);
        assert_eq!(detail(d, 250.0, 255.0, 0.0), 0.0);
        assert!(detail(d, 1200.0, 0.0, 0.0).abs() < 7_000.0);
    }

    #[test]
    fn mars_highlands_have_kilometer_scale_relief() {
        let latitude = 0.3_f64;
        let mut minimum = f64::MAX;
        let mut maximum = f64::MIN;
        for x in 0..20 {
            for z in 0..20 {
                let direction = DVec3::new(
                    (f64::from(x) - 10.0) * 22_000.0 / RADIUS,
                    latitude,
                    1.0 + (f64::from(z) - 10.0) * 22_000.0 / RADIUS,
                )
                .normalize();
                let height = detail(direction, 1200.0, 0.0, 0.0);
                minimum = minimum.min(height);
                maximum = maximum.max(height);
            }
        }
        assert!(
            maximum - minimum > 600.0,
            "insufficient mountains: {minimum}..{maximum}"
        );
    }

    #[test]
    fn local_mountain_detail_changes_at_kilometer_distances() {
        let center = DVec3::new(-0.35, 0.83, 0.44).normalize();
        let east = center.cross(DVec3::Y).normalize();
        let north = east.cross(center);
        let mut minimum = f64::INFINITY;
        let mut maximum = f64::NEG_INFINITY;
        for x in -8..=8 {
            for y in -8..=8 {
                let up = (center
                    + east * (f64::from(x) * 1_500.0 / RADIUS)
                    + north * (f64::from(y) * 1_500.0 / RADIUS))
                    .normalize();
                let height = detail(up, 850.0, 0.0, 0.0);
                minimum = minimum.min(height);
                maximum = maximum.max(height);
            }
        }
        assert!(
            maximum - minimum > 120.0,
            "local highlands have no meaningful relief"
        );
    }

    #[test]
    fn physical_detail_is_not_tied_to_source_texel_size() {
        let d = DVec3::new(-0.35, 0.83, 0.43).normalize();
        let a = detail(d, 1500.0, 0.0, 0.0);
        let b = detail(
            (d + DVec3::new(1.0, -1.0, 0.0) * (250.0 / RADIUS)).normalize(),
            1500.0,
            0.0,
            0.0,
        );
        assert_ne!(a, b);
    }
}

//! Deterministic, world-space terrain detail layered over baked planetary heights.
//!
//! The six baked height faces encode continents and basins. Their resolution
//! must not limit close-range terrain: this function is continuous across
//! cube-face/tile boundaries and depends on *physical* coordinates, not tiles.
//! Keeping this in world-core makes collision, vegetation and rendering agree.

use bevy_math::DVec3;

use crate::planet::RADIUS;

/// Scale-aware relief layered onto the planet's low-resolution global heightmap.
///
/// Continents and water basins come from the baked atlas, but highlands,
/// mountain ridges, foothills, valleys and small terrain features are
/// reconstructed continuously in physical meters (not from the atlas pixels).
/// The evaluated height is identical on both sides of a terrain chunk seam.
pub(crate) fn detail(direction: DVec3, base_height: f64, river_coverage: f64) -> f64 {
    let river = 1.0 - smoothstep(22.0, 200.0, river_coverage);
    if base_height <= 1.5 || river == 0.0 {
        return 0.0;
    }
    let point = direction.normalize() * RADIUS;
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
    broad + local + (coastal_hills + dunes) * river
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
        let a = detail(d, 1200.0, 0.0);
        assert_eq!(a, detail(d, 1200.0, 0.0));
        let east = d.cross(DVec3::Y).normalize();
        let b = detail((d + east * (0.01 / RADIUS)).normalize(), 1200.0, 0.0);
        assert!((b - a).abs() < 0.15, "detail discontinuity: {a} vs {b}");
    }

    #[test]
    fn coastline_and_channels_are_undisturbed() {
        let d = DVec3::new(0.23, 0.95, -0.14).normalize();
        assert_eq!(detail(d, 0.0, 0.0), 0.0);
        assert_eq!(detail(d, -1_500.0, 0.0), 0.0);
        assert_eq!(detail(d, 250.0, 255.0), 0.0);
        assert!(detail(d, 1200.0, 0.0).abs() < 7_000.0);
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
                let height = detail(direction, 1200.0, 0.0);
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
                let height = detail(up, 850.0, 0.0);
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
        let a = detail(d, 1500.0, 0.0);
        let b = detail(
            (d + DVec3::new(1.0, -1.0, 0.0) * (250.0 / RADIUS)).normalize(),
            1500.0,
            0.0,
        );
        assert_ne!(a, b);
    }
}

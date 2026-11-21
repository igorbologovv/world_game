//! Deterministic, world-space terrain detail layered over baked planetary heights.
//!
//! The six baked height faces encode continents and basins. Their resolution
//! must not limit close-range terrain: this function is continuous across
//! cube-face/tile boundaries and depends on *physical* coordinates, not tiles.
//! Keeping this in world-core makes collision, vegetation and rendering agree.

use bevy_math::DVec3;

use crate::planet::RADIUS;

/// Fine relief at physical wavelengths from 16 km down to 36 m.
///
/// The fade at coasts and baked riverbeds intentionally reserves those regions
/// for the future hydrology/shoreline rebuild. It also prevents the existing
/// river surfaces from being buried beneath generated hills.
pub(crate) fn detail(direction: DVec3, base_height: f64, river_coverage: f64) -> f64 {
    // Keep legacy bathymetry and coastline geometry untouched until hydrology is rebuilt.
    let coast = smoothstep(65.0, 350.0, base_height);
    let river = 1.0 - smoothstep(35.0, 170.0, river_coverage);
    let weight = coast * river;
    if weight <= 0.0 {
        return 0.0;
    }

    // Absolute coordinates in meters ensure the same point returns the same
    // height regardless of the camera, streaming tile, or cube face.
    let point = direction.normalize() * RADIUS;
    let broad = value_noise(point / 16_000.0, 0x9B5D_2F31) * 240.0;
    let regional = value_noise(point / 4_000.0, 0xB83A_FD07) * 82.0;
    let rolling = value_noise(point / 900.0, 0x5BD1_8A19) * 25.0;
    let ground = value_noise(point / 180.0, 0xA316_D13B) * 4.5;
    let micro = value_noise(point / 36.0, 0xFE21_03AB) * 0.85;
    (broad + regional + rolling + ground + micro) * weight
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
    let x = p.x.floor() as i64;
    let y = p.y.floor() as i64;
    let z = p.z.floor() as i64;
    let f = p - p.floor();
    let t = f * f * (DVec3::splat(3.0) - 2.0 * f);
    let mut sum = 0.0;
    for dz in 0..=1 {
        for dy in 0..=1 {
            for dx in 0..=1 {
                let weight = (if dx == 0 { 1.0 - t.x } else { t.x })
                    * (if dy == 0 { 1.0 - t.y } else { t.y })
                    * (if dz == 0 { 1.0 - t.z } else { t.z });
                sum += weight * lattice_hash(x + dx, y + dy, z + dz, seed);
            }
        }
    }
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
        assert!(detail(d, 1200.0, 0.0).abs() < 360.0);
    }

    #[test]
    fn physical_detail_is_not_tied_to_source_texel_size() {
        let d = DVec3::new(-0.35, 0.83, 0.43).normalize();
        let a = detail(d, 1500.0, 0.0);
        let b = detail((d + DVec3::new(1.0, -1.0, 0.0) * (250.0 / RADIUS)).normalize(), 1500.0, 0.0);
        assert_ne!(a, b);
    }
}

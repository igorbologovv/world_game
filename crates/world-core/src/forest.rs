//! Physical-scale woodland, meadow, and grove fields.

use bevy_math::DVec3;

use crate::planet::RADIUS;

/// Deterministic woodland density before climate, slope, and water exclusions.
///
/// The direction is planet-local and the height is in meters. The field is
/// anchored to physical coordinates and shared by tree placement and terrain.
pub fn forest_density(direction: DVec3, height: f64) -> f32 {
    let (woodland, clearing, grove, treeline) = forest_factors(direction, height);
    density(woodland, clearing, grove, treeline)
}

/// Woodland coverage samples for terrain shading at two screen footprints.
///
/// The first sample is the placement field. The second averages out the local
/// grove band while retaining the regional woodland and meadow shapes.
pub fn forest_density_bands(direction: DVec3, height: f64) -> [f32; 2] {
    let (woodland, clearing, grove, treeline) = forest_factors(direction, height);
    [
        density(woodland, clearing, grove, treeline),
        density(woodland, clearing, 0.60, treeline),
    ]
}

fn forest_factors(direction: DVec3, height: f64) -> (f64, f64, f64, f64) {
    let p = direction.normalize() * RADIUS;
    // Large woodland regions, smaller meadow clearings and local stands are
    // sampled at physical distances, regardless of the planet's radius.
    // Give climate-eligible land a *small* background of scattered trees:
    // multiplying zero-valued masks previously created enormous dead zones.
    let region = forest_field(p / 22_000.0, 0xB1D2_8491);
    let meadow = forest_field(p / 3_800.0, 0x9284_F505);
    let stand = forest_field(p / 620.0, 0x61A7_96F4);
    let woodland = 0.15 + 0.85 * forest_smoothstep(-0.30, 0.25, region);
    let clearing = 0.08 + 0.92 * forest_smoothstep(-0.36, 0.20, meadow);
    let grove = 0.08 + 0.92 * forest_smoothstep(-0.42, 0.25, stand);
    let treeline = 1.0 - forest_smoothstep(1_500.0, 2_600.0, height);
    (woodland, clearing, grove, treeline)
}

fn density(woodland: f64, clearing: f64, grove: f64, treeline: f64) -> f32 {
    (0.025 + 0.75 * woodland * clearing * grove) as f32 * treeline as f32
}

fn forest_smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn forest_hash(x: i64, y: i64, z: i64, seed: u64) -> f64 {
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

/// Smooth seeded vegetation noise in `[-1, 1]`.
///
/// Divide a planet-local position by the desired feature size before sampling.
pub fn forest_field(p: DVec3, seed: u64) -> f64 {
    let base = p.floor();
    let f = p - base;
    let t = f * f * (DVec3::splat(3.0) - f * 2.0);
    let x = base.x as i64;
    let y = base.y as i64;
    let z = base.z as i64;
    let mut value = 0.0;
    for dz in 0..=1 {
        for dy in 0..=1 {
            for dx in 0..=1 {
                let weight = (if dx == 0 { 1.0 - t.x } else { t.x })
                    * (if dy == 0 { 1.0 - t.y } else { t.y })
                    * (if dz == 0 { 1.0 - t.z } else { t.z });
                value += weight * forest_hash(x + dx, y + dy, z + dz, seed);
            }
        }
    }
    value
}

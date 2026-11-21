//! Physical-scale woodland, meadow, and grove fields.

use bevy_math::{DVec3, Vec3};

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

/// Broad forest-canopy landcover for distant terrain, **not** tree density.
///
/// A forest biome stays a continuous crown-colored surface after individual
/// 3D trees / impostors have been culled. Unlike `forest_density`, this
/// deliberately ignores local meadow/grove thinning and tree quotas.
/// Climate decides whether this is forest-capable land; the 22 km field adds
/// only a very soft regional variation, not thousands of gaps.
pub fn forest_canopy_region(direction: DVec3, height: f64) -> f32 {
    if height <= 3.5 || direction == DVec3::ZERO || !direction.is_finite() {
        return 0.0;
    }
    let dir = direction.normalize().as_vec3();
    // Match the climatic inputs of select_species / spherical.wesl. This
    // evaluation is per terrain vertex, not per pixel and not per tree.
    let weather = climate_noise(dir * 5.8 + Vec3::new(17.3, -4.7, 11.8));
    let rain = climate_noise(dir * 17.0 + Vec3::new(-2.1, 9.4, 4.6));
    let moisture = (0.50 + 0.96 * weather + 0.32 * rain).clamp(0.0, 1.0);
    let temp = (1.07 - 0.78 * dir.y.abs() - (height.max(0.0) as f32) / 16_000.0
        + 0.13 * climate_noise(dir * 4.1 + Vec3::new(3.2, 18.6, -8.4)))
        .clamp(0.0, 1.0);

    // Overlapping ecological ranges avoid hard borders between oak, pine,
    // acacia, jungle and palm areas. These are forest *regions*, not sites.
    let jungle = fade(0.63, 0.72, temp) * fade(0.39, 0.50, moisture);
    let temperate = fade(0.25, 0.33, temp)
        * (1.0 - fade(0.66, 0.75, temp))
        * fade(0.32, 0.43, moisture);
    let boreal = fade(0.10, 0.17, temp)
        * (1.0 - fade(0.45, 0.53, temp))
        * fade(0.19, 0.28, moisture);
    let acacia = fade(0.61, 0.69, temp)
        * fade(0.13, 0.22, moisture)
        * (1.0 - fade(0.64, 0.75, moisture));
    let palm = fade(0.69, 0.77, temp) * fade(0.20, 0.30, moisture)
        * (1.0 - fade(400.0, 550.0, height as f32));
    let habitat = jungle.max(temperate).max(boreal).max(acacia).max(palm);
    let treeline = 1.0 - fade(2_350.0, 2_750.0, height as f32);

    // Do not multiply by the local density or grove mask! They determine
    // tree instances only. The distant canopy must not acquire bare holes.
    let region = forest_field(direction.normalize() * RADIUS / 22_000.0, 0xB1D2_8491);
    let regional_lightness = 0.94 + 0.06 * forest_smoothstep(-0.5, 0.3, region);
    (habitat * treeline * regional_lightness as f32).clamp(0.0, 1.0)
}

fn fade(low: f32, high: f32, value: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn climate_hash(p: Vec3) -> f32 {
    let scaled = p * Vec3::new(0.1031, 0.1030, 0.0973);
    let mut h = scaled - scaled.floor();
    h += Vec3::splat(h.dot(Vec3::new(h.y, h.x, h.z) + Vec3::splat(33.33)));
    let v = (h.x + h.y) * h.z;
    v - v.floor()
}

fn climate_noise(position: Vec3) -> f32 {
    let cell = position.floor();
    let t = position - cell;
    let w = t * t * (Vec3::splat(3.0) - 2.0 * t);
    let near = (climate_hash(cell) + (climate_hash(cell + Vec3::X) - climate_hash(cell)) * w.x)
        + ((climate_hash(cell + Vec3::Y)
                + (climate_hash(cell + Vec3::X + Vec3::Y)
                    - climate_hash(cell + Vec3::Y)) * w.x)
            - (climate_hash(cell)
                + (climate_hash(cell + Vec3::X) - climate_hash(cell)) * w.x))
            * w.y;
    let far_cell = cell + Vec3::Z;
    let far = (climate_hash(far_cell)
        + (climate_hash(far_cell + Vec3::X) - climate_hash(far_cell)) * w.x)
        + ((climate_hash(far_cell + Vec3::Y)
                + (climate_hash(far_cell + Vec3::X + Vec3::Y)
                    - climate_hash(far_cell + Vec3::Y)) * w.x)
            - (climate_hash(far_cell)
                + (climate_hash(far_cell + Vec3::X) - climate_hash(far_cell)) * w.x))
            * w.y;
    near + (far - near) * w.z - 0.5
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

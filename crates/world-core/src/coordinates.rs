//! Cube-sphere coordinates shared by baking, height queries, and renderers.

use bevy_math::{DVec2, DVec3};

use crate::planet::HEIGHT_MAP_SIZE;

/// Warp coefficient of the existing cube-sphere projection.
pub const CUBE_CORRECTION: f64 = 0.87 * 0.87;
const LEGACY_TILE_SIZE: u32 = 512;
const LEGACY_TILE_BORDER: u32 = 2;
const LEGACY_FINE_TILE_COUNT: u32 = 1 << (5 - 1);
const TEXEL_CENTER_OFFSET: f64 =
    0.5 / ((LEGACY_TILE_SIZE - 2 * LEGACY_TILE_BORDER) * LEGACY_FINE_TILE_COUNT) as f64;

/// Align a generated source direction with the baked terrain height samples.
pub fn terrain_aligned_direction(direction: DVec3) -> DVec3 {
    let (face, source) = surface_uv(direction);
    let size = f64::from(HEIGHT_MAP_SIZE);
    let baked = (source * (size - 1.0) + 0.5) / size + TEXEL_CENTER_OFFSET;
    cube_direction(face, baked.clamp(DVec2::ZERO, DVec2::ONE))
}

/// Convert a legacy cube face index (0–5) and normalized face UV to a unit direction.
pub fn cube_direction(face: usize, uv: DVec2) -> DVec3 {
    let uv = uv * 2.0 - 1.0;
    let denominator = 1.0 + CUBE_CORRECTION - CUBE_CORRECTION * uv * uv;
    let plane = uv / DVec2::new(denominator.x.sqrt(), denominator.y.sqrt());
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

/// Convert baked cube-face UVs to normalized source texel coordinates.
pub fn height_source_uv(uv: DVec2, dimensions: DVec2) -> DVec2 {
    let pixel = ((uv - TEXEL_CENTER_OFFSET) * dimensions - 0.5).max(DVec2::ZERO);
    let last = (dimensions - 1.0).max(DVec2::ONE);
    (pixel / last).min(DVec2::ONE)
}

/// Select the dominant cube face and project a finite, nonzero radial vector onto it.
pub fn surface_uv(direction: DVec3) -> (usize, DVec2) {
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
    let correction = (1.0 + CUBE_CORRECTION) / (1.0 + CUBE_CORRECTION * plane * plane);
    let corrected = plane * DVec2::new(correction.x.sqrt(), correction.y.sqrt());
    let uv = (corrected * 0.5 + 0.5).clamp(DVec2::ZERO, DVec2::ONE);
    (face, uv)
}

/// Convert source heightmap UVs to a unit direction using the original scalar bake math.
pub fn source_direction(face: usize, u: f64, v: f64) -> [f64; 3] {
    let mut u = (u - 0.5) / 0.5;
    let mut v = (v - 0.5) / 0.5;

    u /= (1.0 + CUBE_CORRECTION - CUBE_CORRECTION * u * u).sqrt();
    v /= (1.0 + CUBE_CORRECTION - CUBE_CORRECTION * v * v).sqrt();

    let p = match face {
        0 => [-1.0, -v, u],
        1 => [u, -v, 1.0],
        2 => [u, 1.0, v],
        3 => [1.0, -u, v],
        4 => [v, -u, -1.0],
        5 => [v, -1.0, u],
        _ => unreachable!(),
    };

    normalize(p)
}

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();

    [v[0] / len, v[1] / len, v[2] / len]
}

/// Project a source direction onto a specific face for atlas baking, including across edges.
pub fn source_face_uv(face: usize, [x, y, z]: [f64; 3]) -> [f64; 2] {
    let plane = match face {
        0 => [z / -x, -y / -x],
        1 => [x / z, -y / z],
        2 => [x / y, z / y],
        3 => [-y / x, z / x],
        4 => [-y / -z, x / -z],
        5 => [z / -y, x / -y],
        _ => unreachable!(),
    };
    plane.map(|axis| {
        0.5 + 0.5 * axis * ((1.0 + CUBE_CORRECTION) / (1.0 + CUBE_CORRECTION * axis * axis)).sqrt()
    })
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

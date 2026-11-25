//! Moisture around water surfaces at terrain mesh vertices.

use bevy::math::DVec3;
use world_core::{
    planet::{RADIUS, WATER_SURFACE_ELEVATION},
    LocalPool, SurfaceHeights, WaterKind,
};

pub(super) fn influence(
    heights: &SurfaceHeights,
    pools: &[LocalPool],
    up: DVec3,
    height: f64,
) -> f32 {
    let east = up.any_orthonormal_vector();
    let north = up.cross(east);
    let offsets = [east, north, -east, -north];
    let probe = |axis: DVec3, distance: f64| (up + axis * (distance / RADIUS)).normalize();

    let mut fresh = heights.freshwater_influence(up, height);
    let mut submerged = if fresh > 0.79 {
        heights
            .open_water_at(up, height)
            .filter(|water| water.kind != WaterKind::Ocean)
            .map_or(0.0, |water| water.depth)
    } else {
        0.0
    };
    for pool in pools {
        let distance = up.distance(pool.center) * RADIUS;
        if distance < pool.radius + 36.0 {
            let lateral = 1.0 - smoothstep(pool.radius * 0.66, pool.radius + 28.0, distance);
            let vertical = 1.0 - smoothstep(0.08, 2.8, height - pool.elevation);
            fresh = fresh.max(lateral * vertical);
            if distance < pool.radius {
                submerged = submerged.max(pool.elevation - height);
            }
        }
    }

    if submerged > 0.0 {
        // Values below -1 encode wet sediment below freshwater. Interpolate
        // through the saturated bank before selecting submerged bed textures.
        return (-1.0 - smoothstep(0.02, 1.2, submerged)) as f32;
    }
    if height < -9.0 {
        return if fresh > 0.0 { -fresh as f32 } else { 1.0 };
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

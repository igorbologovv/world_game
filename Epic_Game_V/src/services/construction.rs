//! Server-side construction estimate from the shipped planetary height field.

use std::{path::PathBuf, sync::OnceLock};

use bevy_math::DVec3;
use world_core::{
    SurfaceHeights,
    construction::{CONSTRUCTION_BLEND_M, ConstructionGrid},
    coordinates::terrain_aligned_direction,
    planet::WATER_SURFACE_ELEVATION,
};

use crate::{
    db::rows::{BuildingTypeRow, MapRow},
    error::ApiError,
    pb::QuoteBuildingResponse,
};

static HEIGHTS: OnceLock<SurfaceHeights> = OnceLock::new();

fn heights() -> &'static SurfaceHeights {
    HEIGHTS.get_or_init(|| {
        let path = std::env::var_os("WORLD_ASSETS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("../assets"));
        SurfaceHeights::load(&path)
    })
}

pub fn estimate(
    map: &MapRow,
    tile_x: i32,
    tile_y: i32,
    building: &BuildingTypeRow,
) -> Result<QuoteBuildingResponse, ApiError> {
    let source = DVec3::new(
        map.origin_x.ok_or_else(|| {
            ApiError::conflict("map_has_no_planet_grid", "map has no planetary grid")
        })?,
        map.origin_y.ok_or_else(|| {
            ApiError::conflict("map_has_no_planet_grid", "map has no planetary grid")
        })?,
        map.origin_z.ok_or_else(|| {
            ApiError::conflict("map_has_no_planet_grid", "map has no planetary grid")
        })?,
    );
    let grid = ConstructionGrid {
        origin: terrain_aligned_direction(source),
        origin_cell: map.origin_cell,
    };
    let mut clearance_m2 = 0.0;
    let shoulder = CONSTRUCTION_BLEND_M as i32;
    for y in tile_y - shoulder..tile_y + building.footprint_height + shoulder {
        for x in tile_x - shoulder..tile_x + building.footprint_width + shoulder {
            let east = f64::from(x - map.origin_cell) + 0.5;
            let north = f64::from(y - map.origin_cell) + 0.5;
            if east.hypot(north) > map.clearing_radius_m.unwrap_or(0.0) {
                clearance_m2 += 1.0;
            }
        }
    }
    let mut elevations =
        Vec::with_capacity((building.footprint_width * building.footprint_height) as usize);
    for y in tile_y..tile_y + building.footprint_height {
        for x in tile_x..tile_x + building.footprint_width {
            let direction = grid
                .position_at(f64::from(x) + 0.5, f64::from(y) + 0.5, 0.0)
                .normalize();
            let ground = heights()
                .sample(direction)
                .ok_or_else(|| ApiError::internal("planet height assets are unavailable"))?;
            if !ground.is_finite() || ground <= WATER_SURFACE_ELEVATION {
                return Err(ApiError::conflict(
                    "unsuitable_terrain",
                    "construction requires dry ground",
                ));
            }
            elevations.push(ground);
        }
    }
    let (pad, earthwork_m3) = earthwork(&mut elevations);
    // The authored landing clearing has already removed vegetation. Bill only
    // additional square meters of roots and ground cover cleared by this pad.
    let base = building
        .placement_cost_mt
        .parse::<f64>()
        .map_err(|_| ApiError::internal("invalid base building price"))?;
    let earthwork_cost = (earthwork_m3 * 2.0).ceil();
    let vegetation_cost = clearance_m2;
    let center_x = f64::from(tile_x) + f64::from(building.footprint_width) / 2.0;
    let center_y = f64::from(tile_y) + f64::from(building.footprint_height) / 2.0;
    let center = grid.position_at(center_x, center_y, pad);
    Ok(QuoteBuildingResponse {
        base_cost: format!("{base:.2}"),
        earthwork_cost: format!("{earthwork_cost:.2}"),
        vegetation_cost: format!("{vegetation_cost:.2}"),
        total_cost: format!("{:.2}", base + earthwork_cost + vegetation_cost),
        earthwork_m3,
        clearance_m2,
        pad_elevation: pad,
        planet_x: center.x,
        planet_y: center.y,
        planet_z: center.z,
    })
}

fn earthwork(elevations: &mut [f64]) -> (f64, f64) {
    elevations.sort_by(f64::total_cmp);
    let pad = elevations[elevations.len() / 2];
    let volume = elevations.iter().map(|ground| (ground - pad).abs()).sum();
    (pad, volume)
}

#[cfg(test)]
mod tests {
    use super::earthwork;

    #[test]
    fn flat_land_is_cheaper_than_sloped_land() {
        let (_, flat) = earthwork(&mut [10.0; 25]);
        let (_, slope) = earthwork(&mut [8.0, 9.0, 10.0, 11.0, 12.0]);
        assert_eq!(flat, 0.0);
        assert!(slope > flat);
    }
}

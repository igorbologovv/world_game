use crate::{
    constants::{
        BUILDING_ACTIVE, BUILDING_CONSTRUCTING, BUILDING_DEMOLISHED, BUILDING_HAS_WORKERS,
        BUILDING_UPGRADABLE,
    },
    error::ApiError,
    pb::{BuildBudget, Building},
};
use sqlx::{Row, postgres::PgRow};

#[derive(Debug)]
pub struct MapRow {
    pub id: i64,
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    pub origin_x: Option<f64>,
    pub origin_y: Option<f64>,
    pub origin_z: Option<f64>,
    pub origin_cell: i32,
    pub clearing_radius_m: Option<f64>,
}

#[derive(Debug)]
pub struct BuildingTypeRow {
    pub id: i64,
    pub footprint_width: i32,
    pub footprint_height: i32,
    pub footprint_depth: i32,
    pub placement_cost_mt: String,
}

#[derive(Debug)]
pub struct BudgetRow {
    pub total_cells: u32,
    pub used_cells: u32,
    pub free_cells: u32,
}

pub fn row_to_building(row: &PgRow) -> Building {
    let assigned_workers = row.get("assigned_workers");
    let level = row.get::<i32, _>("level");
    let max_level = row.get::<i32, _>("max_level");
    let status = row.get::<String, _>("status");

    Building {
        id: row.get("id"),
        building_type_code: row.get("building_type_code"),
        map_code: row.get("map_code"),
        tile_x: row.get("tile_x"),
        tile_y: row.get("tile_y"),
        tile_z: row.get("tile_z"),
        footprint_width: row.get("footprint_width"),
        footprint_height: row.get("footprint_height"),
        footprint_depth: row.get("footprint_depth"),
        level,
        assigned_workers,
        flags: building_flags(&status, assigned_workers, level, max_level),
        planet_x: row.try_get("planet_x").unwrap_or(0.0),
        planet_y: row.try_get("planet_y").unwrap_or(0.0),
        planet_z: row.try_get("planet_z").unwrap_or(0.0),
        pad_elevation: row.try_get("pad_elevation").unwrap_or(0.0),
        earthwork_m3: row.try_get("earthwork_m3").unwrap_or(0.0),
        clearance_m2: row.try_get("clearance_m2").unwrap_or(0.0),
    }
}

pub fn building_flags(status: &str, assigned_workers: i32, level: i32, max_level: i32) -> u32 {
    let mut flags = match status {
        "active" => BUILDING_ACTIVE,
        "constructing" => BUILDING_CONSTRUCTING,
        "demolished" => BUILDING_DEMOLISHED,
        _ => 0,
    };

    if assigned_workers > 0 {
        flags |= BUILDING_HAS_WORKERS;
    }

    if status == "active" && level < max_level {
        flags |= BUILDING_UPGRADABLE;
    }

    flags
}

impl BudgetRow {
    pub fn into_proto(self) -> BuildBudget {
        BuildBudget {
            total_cells: self.total_cells,
            used_cells: self.used_cells,
            free_cells: self.free_cells,
        }
    }
}

pub fn building_cells(width: i32, height: i32, depth: i32) -> Result<u32, ApiError> {
    let cells = width as i64 * height as i64 * depth as i64;
    if cells < 0 || cells > u32::MAX as i64 {
        return Err(ApiError::bad_request(
            "invalid_footprint",
            "building footprint is invalid",
        ));
    }

    Ok(cells as u32)
}

pub fn non_negative_i32_to_u32(value: i32) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

pub fn non_negative_i32_to_u64(value: i32) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

pub fn non_negative_i64_to_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub fn non_negative_i64_to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

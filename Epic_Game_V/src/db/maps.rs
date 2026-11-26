use crate::{db::rows::MapRow, error::ApiError};
use sqlx::{Postgres, Row, Transaction};

pub async fn load_map(tx: &mut Transaction<'_, Postgres>, code: &str) -> Result<MapRow, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT id, width, height, depth, origin_x, origin_y, origin_z, origin_cell, clearing_radius_m
        FROM economy_maps
        WHERE code = $1
        "#,
    )
    .bind(code)
    .fetch_optional(&mut **tx)
    .await
    .map_err(ApiError::from_sqlx)?
    .ok_or_else(|| ApiError::not_found("map_not_found", "map not found"))?;

    Ok(MapRow {
        id: row.get("id"),
        width: row.get("width"),
        height: row.get("height"),
        depth: row.get("depth"),
        origin_x: row.get("origin_x"),
        origin_y: row.get("origin_y"),
        origin_z: row.get("origin_z"),
        origin_cell: row.get("origin_cell"),
        clearing_radius_m: row.get("clearing_radius_m"),
    })
}

use crate::{db::rows::BuildingTypeRow, error::ApiError};
use sqlx::{Postgres, Row, Transaction};

pub async fn load_building_type(
    tx: &mut Transaction<'_, Postgres>,
    code: &str,
) -> Result<BuildingTypeRow, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT
            id,
            footprint_width,
            footprint_height,
            footprint_depth,
            placement_cost_mt::text AS placement_cost_mt
        FROM building_type_definitions
        WHERE code = $1
        "#,
    )
    .bind(code)
    .fetch_optional(&mut **tx)
    .await
    .map_err(ApiError::from_sqlx)?
    .ok_or_else(|| ApiError::not_found("building_type_not_found", "building type not found"))?;

    Ok(BuildingTypeRow {
        id: row.get("id"),
        footprint_width: row.get("footprint_width"),
        footprint_height: row.get("footprint_height"),
        footprint_depth: row.get("footprint_depth"),
        placement_cost_mt: row.get("placement_cost_mt"),
    })
}

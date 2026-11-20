use crate::{db::rows::MapRow, error::ApiError};
use sqlx::{Postgres, Row, Transaction};

pub async fn load_map(tx: &mut Transaction<'_, Postgres>, code: &str) -> Result<MapRow, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT id, width, height, depth
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
    })
}

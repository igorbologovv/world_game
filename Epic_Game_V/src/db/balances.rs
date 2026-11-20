use crate::{error::ApiError, pb::Balance};
use sqlx::{PgPool, Row};

pub async fn load_balances(db: &PgPool, uid: &str) -> Result<Vec<Balance>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT
            af.code AS asset_code,
            pab.tier,
            pab.amount::text AS amount
        FROM player_asset_balances pab
        JOIN asset_families af ON af.id = pab.asset_family_id
        WHERE pab.uid = $1
        ORDER BY af.code, pab.tier
        "#,
    )
    .bind(uid)
    .fetch_all(db)
    .await
    .map_err(ApiError::from_sqlx)?;

    Ok(rows
        .iter()
        .map(|row| Balance {
            asset_code: row.get("asset_code"),
            tier: row.get("tier"),
            amount: row.get("amount"),
        })
        .collect())
}

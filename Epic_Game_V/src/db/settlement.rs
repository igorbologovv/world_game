use crate::{db::rows::non_negative_i64_to_u64, error::ApiError, pb::SettlementSummary};
use sqlx::{PgPool, Postgres, Row, Transaction};

pub async fn settle_player(db: &PgPool, uid: &str) -> Result<SettlementSummary, ApiError> {
    let mut tx = db.begin().await.map_err(ApiError::from_sqlx)?;
    let summary = settle_player_in_tx(&mut tx, uid).await?;
    tx.commit().await.map_err(ApiError::from_sqlx)?;

    Ok(summary)
}

pub async fn settle_player_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
) -> Result<SettlementSummary, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT
            settled_hours::bigint AS settled_hours,
            previous_settled_at::text AS previous_settled_at,
            new_settled_at::text AS new_settled_at
        FROM settle_player_economy($1)
        "#,
    )
    .bind(uid)
    .fetch_one(&mut **tx)
    .await
    .map_err(ApiError::from_sqlx)?;

    Ok(SettlementSummary {
        settled_hours: non_negative_i64_to_u64(row.get("settled_hours")),
        previous_settled_at: row.get("previous_settled_at"),
        new_settled_at: row.get("new_settled_at"),
    })
}

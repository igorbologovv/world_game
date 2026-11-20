use crate::error::ApiError;
use sqlx::{Postgres, Transaction};

pub async fn charge_placement_mt(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    placement_cost_mt: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        SELECT balance
        FROM users
        WHERE uid = $1
        FOR UPDATE
        "#,
    )
    .bind(uid)
    .fetch_optional(&mut **tx)
    .await
    .map_err(ApiError::from_sqlx)?
    .ok_or_else(|| ApiError::not_found("user_not_found", "user not found"))?;

    let result = sqlx::query(
        r#"
        UPDATE users
        SET balance = balance - $2::numeric
        WHERE uid = $1
          AND balance >= $2::numeric
        "#,
    )
    .bind(uid)
    .bind(placement_cost_mt)
    .execute(&mut **tx)
    .await
    .map_err(ApiError::from_sqlx)?;

    if result.rows_affected() == 0 {
        return Err(ApiError::conflict(
            "insufficient_mt",
            "not enough MT to place building",
        ));
    }

    Ok(())
}

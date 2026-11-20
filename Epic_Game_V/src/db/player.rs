use crate::error::ApiError;
use sqlx::PgPool;

pub async fn ensure_player_rows(db: &PgPool, uid: &str) -> Result<(), ApiError> {
    let mut tx = db.begin().await.map_err(ApiError::from_sqlx)?;

    // The standalone game owns registration. Match the original starting balance
    // and only provision after Firebase authentication has supplied the UID.
    sqlx::query(
        r#"
        INSERT INTO users (uid, username, email, balance, account_type)
        VALUES ($1, $1, '', 2000, 'standard')
        ON CONFLICT (uid) DO NOTHING
        "#,
    )
    .bind(uid)
    .execute(&mut *tx)
    .await
    .map_err(ApiError::from_sqlx)?;

    sqlx::query(
        r#"
        INSERT INTO player_economy_state (uid)
        VALUES ($1)
        ON CONFLICT (uid) DO NOTHING
        "#,
    )
    .bind(uid)
    .execute(&mut *tx)
    .await
    .map_err(ApiError::from_sqlx)?;

    sqlx::query(
        r#"
        INSERT INTO player_population (uid)
        VALUES ($1)
        ON CONFLICT (uid) DO NOTHING
        "#,
    )
    .bind(uid)
    .execute(&mut *tx)
    .await
    .map_err(ApiError::from_sqlx)?;

    sqlx::query(
        r#"
        INSERT INTO player_building_cell_budgets (uid)
        VALUES ($1)
        ON CONFLICT (uid) DO NOTHING
        "#,
    )
    .bind(uid)
    .execute(&mut *tx)
    .await
    .map_err(ApiError::from_sqlx)?;

    tx.commit().await.map_err(ApiError::from_sqlx)
}

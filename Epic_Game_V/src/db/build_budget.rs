use crate::{
    db::rows::{BudgetRow, non_negative_i32_to_u32, non_negative_i64_to_u32},
    error::ApiError,
};
use sqlx::{PgPool, Postgres, Row, Transaction};

pub async fn load_build_budget(db: &PgPool, uid: &str) -> Result<BudgetRow, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT
            budget.total_cells,
            COALESCE(
                SUM(
                    eb.footprint_width::bigint
                    * eb.footprint_height::bigint
                    * eb.footprint_depth::bigint
                ) FILTER (WHERE eb.status <> 'demolished'),
                0
            )::bigint AS used_cells
        FROM player_building_cell_budgets budget
        LEFT JOIN economy_buildings eb ON eb.owner_uid = budget.uid
        WHERE budget.uid = $1
        GROUP BY budget.total_cells
        "#,
    )
    .bind(uid)
    .fetch_one(db)
    .await
    .map_err(ApiError::from_sqlx)?;

    let total_cells = non_negative_i32_to_u32(row.get("total_cells"));
    let used_cells = non_negative_i64_to_u32(row.get("used_cells"));
    let free_cells = total_cells.saturating_sub(used_cells);

    Ok(BudgetRow {
        total_cells,
        used_cells,
        free_cells,
    })
}

pub async fn lock_build_budget(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
) -> Result<BudgetRow, ApiError> {
    sqlx::query(
        r#"
        SELECT uid
        FROM player_building_cell_budgets
        WHERE uid = $1
        FOR UPDATE
        "#,
    )
    .bind(uid)
    .fetch_one(&mut **tx)
    .await
    .map_err(ApiError::from_sqlx)?;

    let row = sqlx::query(
        r#"
        SELECT
            budget.total_cells,
            COALESCE(
                SUM(
                    eb.footprint_width::bigint
                    * eb.footprint_height::bigint
                    * eb.footprint_depth::bigint
                ) FILTER (WHERE eb.status <> 'demolished'),
                0
            )::bigint AS used_cells
        FROM player_building_cell_budgets budget
        LEFT JOIN economy_buildings eb ON eb.owner_uid = budget.uid
        WHERE budget.uid = $1
        GROUP BY budget.total_cells
        "#,
    )
    .bind(uid)
    .fetch_one(&mut **tx)
    .await
    .map_err(ApiError::from_sqlx)?;

    let total_cells = non_negative_i32_to_u32(row.get("total_cells"));
    let used_cells = non_negative_i64_to_u32(row.get("used_cells"));
    let free_cells = total_cells.saturating_sub(used_cells);

    Ok(BudgetRow {
        total_cells,
        used_cells,
        free_cells,
    })
}

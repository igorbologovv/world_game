use crate::{
    db::rows::{non_negative_i32_to_u64, non_negative_i64_to_u32},
    error::{ApiError, map_population_increase_error},
    pb::{Balance, IncreasePopulationResponse, Population},
};
use sqlx::{PgPool, Row};

pub async fn load_population(db: &PgPool, uid: &str) -> Result<Population, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT total_population
        FROM player_population
        WHERE uid = $1
        "#,
    )
    .bind(uid)
    .fetch_one(db)
    .await
    .map_err(ApiError::from_sqlx)?;

    Ok(Population {
        total_population: non_negative_i32_to_u64(row.get("total_population")),
    })
}

#[allow(deprecated)] // Populate legacy protobuf fields during the KE-TSE transition.
pub async fn increase_player_population(
    db: &PgPool,
    uid: &str,
    amount: i32,
) -> Result<IncreasePopulationResponse, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT
            total_population,
            added_population,
            mt_cost::text AS mt_cost,
            food_cost::text AS food_cost,
            remaining_mt_balance::text AS remaining_mt_balance,
            remaining_food_balance::text AS remaining_food_balance
        FROM increase_player_population($1, $2)
        "#,
    )
    .bind(uid)
    .bind(amount)
    .fetch_one(db)
    .await
    .map_err(map_population_increase_error)?;

    Ok(IncreasePopulationResponse {
        population: Some(Population {
            total_population: non_negative_i32_to_u64(row.get("total_population")),
        }),
        added_population: non_negative_i64_to_u32(i64::from(row.get::<i32, _>("added_population"))),
        mt_cost: row.get("mt_cost"),
        ke_tse_cost: row.get("mt_cost"),
        food_cost: row.get("food_cost"),
        remaining_mt_balance: row.get("remaining_mt_balance"),
        remaining_ke_tse_balance: row.get("remaining_mt_balance"),
        food_balance: Some(Balance {
            asset_code: "food".to_string(),
            tier: 0,
            amount: row.get("remaining_food_balance"),
        }),
    })
}

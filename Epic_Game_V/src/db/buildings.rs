use crate::{
    db::rows::{BuildingTypeRow, MapRow, row_to_building},
    error::ApiError,
    pb::{Building, PlaceBuildingRequest},
};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};

#[derive(Debug)]
pub struct WorkerAssignmentContext {
    pub max_workers: Option<i32>,
    pub other_assigned_workers: i64,
    pub total_population: i32,
}

#[derive(Debug)]
pub struct UpgradeCostRule {
    pub cost_kind: String,
    pub asset_family_id: Option<i64>,
    pub min_tier: Option<i32>,
    pub amount: String,
    pub amount_is_positive: bool,
}

pub async fn load_buildings(db: &PgPool, uid: &str) -> Result<Vec<Building>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT
            eb.id::text AS id,
            btd.code AS building_type_code,
            em.code AS map_code,
            eb.tile_x,
            eb.tile_y,
            eb.tile_z,
            eb.footprint_width,
            eb.footprint_height,
            eb.footprint_depth,
            eb.level,
            eb.assigned_workers,
            eb.status,
            btd.max_level
        FROM economy_buildings eb
        JOIN building_type_definitions btd ON btd.id = eb.building_type_id
        JOIN economy_maps em ON em.id = eb.map_id
        WHERE eb.owner_uid = $1
        ORDER BY eb.created_at, eb.id
        "#,
    )
    .bind(uid)
    .fetch_all(db)
    .await
    .map_err(ApiError::from_sqlx)?;

    Ok(rows.iter().map(row_to_building).collect())
}

pub async fn insert_building(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    map: &MapRow,
    building_type: &BuildingTypeRow,
    request: &PlaceBuildingRequest,
) -> Result<PgRow, sqlx::Error> {
    sqlx::query(
        r#"
        WITH inserted AS (
            INSERT INTO economy_buildings (
                owner_uid,
                map_id,
                building_type_id,
                tile_x,
                tile_y,
                tile_z,
                footprint_width,
                footprint_height,
                footprint_depth
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING *
        )
        SELECT
            inserted.id::text AS id,
            btd.code AS building_type_code,
            em.code AS map_code,
            inserted.tile_x,
            inserted.tile_y,
            inserted.tile_z,
            inserted.footprint_width,
            inserted.footprint_height,
            inserted.footprint_depth,
            inserted.level,
            inserted.assigned_workers,
            inserted.status,
            btd.max_level
        FROM inserted
        JOIN building_type_definitions btd ON btd.id = inserted.building_type_id
        JOIN economy_maps em ON em.id = inserted.map_id
        "#,
    )
    .bind(uid)
    .bind(map.id)
    .bind(building_type.id)
    .bind(request.tile_x)
    .bind(request.tile_y)
    .bind(request.tile_z)
    .bind(building_type.footprint_width)
    .bind(building_type.footprint_height)
    .bind(building_type.footprint_depth)
    .fetch_one(&mut **tx)
    .await
}

pub async fn set_workers(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    building_id: &str,
    assigned_workers: i32,
) -> Result<PgRow, sqlx::Error> {
    sqlx::query(
        r#"
        WITH updated AS (
            UPDATE economy_buildings
            SET assigned_workers = $3,
                updated_at = now()
            WHERE id = $1::uuid
              AND owner_uid = $2
              AND status = 'active'
            RETURNING *
        )
        SELECT
            updated.id::text AS id,
            btd.code AS building_type_code,
            em.code AS map_code,
            updated.tile_x,
            updated.tile_y,
            updated.tile_z,
            updated.footprint_width,
            updated.footprint_height,
            updated.footprint_depth,
            updated.level,
            updated.assigned_workers,
            updated.status,
            btd.max_level
        FROM updated
        JOIN building_type_definitions btd ON btd.id = updated.building_type_id
        JOIN economy_maps em ON em.id = updated.map_id
        "#,
    )
    .bind(building_id)
    .bind(uid)
    .bind(assigned_workers)
    .fetch_one(&mut **tx)
    .await
}

pub async fn load_worker_assignment_context(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    building_id: &str,
) -> Result<Option<WorkerAssignmentContext>, sqlx::Error> {
    let building = sqlx::query(
        r#"
        SELECT
            eb.id::text AS id,
            btd.code AS building_type_code,
            em.code AS map_code,
            eb.tile_x,
            eb.tile_y,
            eb.tile_z,
            eb.footprint_width,
            eb.footprint_height,
            eb.footprint_depth,
            eb.level,
            eb.assigned_workers,
            eb.status,
            btd.max_level,
            (btd.max_workers + (2 * (eb.level - 1))) AS max_workers
        FROM economy_buildings eb
        JOIN building_type_definitions btd ON btd.id = eb.building_type_id
        JOIN economy_maps em ON em.id = eb.map_id
        WHERE eb.id = $1::uuid
          AND eb.owner_uid = $2
          AND eb.status = 'active'
        FOR UPDATE OF eb
        "#,
    )
    .bind(building_id)
    .bind(uid)
    .fetch_optional(&mut **tx)
    .await?;

    let Some(building) = building else {
        return Ok(None);
    };

    sqlx::query(
        r#"
        SELECT id
        FROM economy_buildings
        WHERE owner_uid = $1
          AND status = 'active'
        FOR UPDATE
        "#,
    )
    .bind(uid)
    .fetch_all(&mut **tx)
    .await?;

    let population = sqlx::query(
        r#"
        SELECT total_population
        FROM player_population
        WHERE uid = $1
        FOR UPDATE
        "#,
    )
    .bind(uid)
    .fetch_one(&mut **tx)
    .await?;

    let assignment = sqlx::query(
        r#"
        SELECT COALESCE(SUM(assigned_workers), 0)::bigint AS other_assigned_workers
        FROM economy_buildings
        WHERE owner_uid = $1
          AND id <> $2::uuid
          AND status = 'active'
        "#,
    )
    .bind(uid)
    .bind(building_id)
    .fetch_one(&mut **tx)
    .await?;

    Ok(Some(WorkerAssignmentContext {
        max_workers: building.get("max_workers"),
        other_assigned_workers: assignment.get("other_assigned_workers"),
        total_population: population.get("total_population"),
    }))
}

pub async fn load_upgradable_building(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    building_id: &str,
) -> Result<Option<PgRow>, sqlx::Error> {
    sqlx::query(
        r#"
        SELECT
            eb.id::text AS id,
            btd.code AS building_type_code,
            em.code AS map_code,
            eb.tile_x,
            eb.tile_y,
            eb.tile_z,
            eb.footprint_width,
            eb.footprint_height,
            eb.footprint_depth,
            eb.level,
            eb.assigned_workers,
            eb.status,
            btd.max_level,
            eb.building_type_id
        FROM economy_buildings eb
        JOIN building_type_definitions btd ON btd.id = eb.building_type_id
        JOIN economy_maps em ON em.id = eb.map_id
        WHERE eb.id = $1::uuid
          AND eb.owner_uid = $2
          AND eb.status = 'active'
          AND eb.level < btd.max_level
        FOR UPDATE OF eb
        "#,
    )
    .bind(building_id)
    .bind(uid)
    .fetch_optional(&mut **tx)
    .await
}

pub async fn load_upgrade_cost_rules(
    tx: &mut Transaction<'_, Postgres>,
    building_type_id: i64,
    current_level: i32,
) -> Result<Vec<UpgradeCostRule>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT
            cost_kind,
            asset_family_id,
            min_tier,
            amount::text AS amount,
            amount > 0 AS amount_is_positive
        FROM (
            SELECT
                cost_kind,
                asset_family_id,
                min_tier,
                base_amount * POWER(growth_rate, ($2 - applies_from_level)::numeric) AS amount,
                applies_from_level,
                id
            FROM building_upgrade_cost_rules
            WHERE building_type_id = $1
              AND applies_from_level = (
                  SELECT MAX(applies_from_level)
                  FROM building_upgrade_cost_rules
                  WHERE building_type_id = $1
                    AND applies_from_level <= $2
              )
        ) selected_rules
        ORDER BY applies_from_level, id
        "#,
    )
    .bind(building_type_id)
    .bind(current_level)
    .fetch_all(&mut **tx)
    .await?;

    Ok(rows
        .iter()
        .map(|row| UpgradeCostRule {
            cost_kind: row.get("cost_kind"),
            asset_family_id: row.get("asset_family_id"),
            min_tier: row.get("min_tier"),
            amount: row.get("amount"),
            amount_is_positive: row.get("amount_is_positive"),
        })
        .collect())
}

pub async fn spend_upgrade_mt(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    amount: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE users
        SET balance = balance - $2::numeric
        WHERE uid = $1
          AND balance >= $2::numeric
        "#,
    )
    .bind(uid)
    .bind(amount)
    .execute(&mut **tx)
    .await?;

    Ok(result.rows_affected() == 1)
}

pub async fn spend_upgrade_exact_asset(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    asset_family_id: i64,
    tier: i32,
    amount: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE player_asset_balances
        SET amount = amount - $4::numeric,
            updated_at = now()
        WHERE uid = $1
          AND asset_family_id = $2
          AND tier = $3
          AND amount >= $4::numeric
        "#,
    )
    .bind(uid)
    .bind(asset_family_id)
    .bind(tier)
    .bind(amount)
    .execute(&mut **tx)
    .await?;

    Ok(result.rows_affected() == 1)
}

pub async fn spend_upgrade_category_points(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    asset_family_id: i64,
    min_tier: i32,
    required_points: &str,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query(
        r#"
        WITH locked AS (
            SELECT
                pab.uid,
                pab.asset_family_id,
                pab.tier,
                pab.amount,
                aqr.base_points * POWER(aqr.growth_rate, (pab.tier - 1)::numeric) AS points_per_unit
            FROM player_asset_balances pab
            JOIN asset_quality_rules aqr ON aqr.asset_family_id = pab.asset_family_id
            WHERE pab.uid = $1
              AND pab.asset_family_id = $2
              AND pab.tier >= $3
              AND pab.amount > 0
            FOR UPDATE OF pab
        ),
        total AS (
            SELECT COALESCE(SUM(amount * points_per_unit), 0)::numeric AS available_points
            FROM locked
        ),
        spend_plan AS (
            SELECT
                uid,
                asset_family_id,
                tier,
                LEAST(
                    amount,
                    GREATEST(
                        $4::numeric
                            - COALESCE(
                                SUM(amount * points_per_unit) OVER (
                                    ORDER BY tier
                                    ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING
                                ),
                                0
                            ),
                        0
                    ) / points_per_unit
                ) AS amount_to_spend
            FROM locked
            WHERE (SELECT available_points FROM total) >= $4::numeric
        ),
        spent AS (
            UPDATE player_asset_balances pab
            SET amount = pab.amount - spend_plan.amount_to_spend,
                updated_at = now()
            FROM spend_plan
            WHERE pab.uid = spend_plan.uid
              AND pab.asset_family_id = spend_plan.asset_family_id
              AND pab.tier = spend_plan.tier
              AND spend_plan.amount_to_spend > 0
            RETURNING spend_plan.amount_to_spend
        )
        SELECT
            (SELECT available_points FROM total) >= $4::numeric AS enough,
            COALESCE(SUM(amount_to_spend), 0)::text AS spent_amount
        FROM spent
        "#,
    )
    .bind(uid)
    .bind(asset_family_id)
    .bind(min_tier)
    .bind(required_points)
    .fetch_one(&mut **tx)
    .await?;

    Ok(row.get("enough"))
}

pub async fn increment_building_level(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    building_id: &str,
) -> Result<Option<PgRow>, sqlx::Error> {
    sqlx::query(
        r#"
        WITH updated AS (
            UPDATE economy_buildings eb
            SET level = eb.level + 1,
                updated_at = now()
            FROM building_type_definitions btd
            WHERE eb.id = $1::uuid
              AND eb.owner_uid = $2
              AND eb.status = 'active'
              AND btd.id = eb.building_type_id
              AND eb.level < btd.max_level
            RETURNING eb.*
        )
        SELECT
            updated.id::text AS id,
            btd.code AS building_type_code,
            em.code AS map_code,
            updated.tile_x,
            updated.tile_y,
            updated.tile_z,
            updated.footprint_width,
            updated.footprint_height,
            updated.footprint_depth,
            updated.level,
            updated.assigned_workers,
            updated.status,
            btd.max_level
        FROM updated
        JOIN building_type_definitions btd ON btd.id = updated.building_type_id
        JOIN economy_maps em ON em.id = updated.map_id
        "#,
    )
    .bind(building_id)
    .bind(uid)
    .fetch_optional(&mut **tx)
    .await
}

use crate::{
    db,
    db::rows::{BuildingTypeRow, MapRow, building_cells, row_to_building},
    error::{ApiError, map_place_building_error, map_uuid_error},
    pb::{
        PlaceBuildingRequest, PlaceBuildingResponse, QuoteBuildingRequest, QuoteBuildingResponse,
        SetWorkersRequest, SetWorkersResponse, UpgradeBuildingRequest, UpgradeBuildingResponse,
    },
    services,
};
use sqlx::{PgPool, Row};

pub async fn quote_building(
    db: &PgPool,
    uid: &str,
    request: QuoteBuildingRequest,
) -> Result<QuoteBuildingResponse, ApiError> {
    services::player::ensure_player_rows(db, uid).await?;
    let mut tx = db.begin().await.map_err(ApiError::from_sqlx)?;
    let map = db::maps::load_map(&mut tx, &request.map_code).await?;
    let building =
        db::building_types::load_building_type(&mut tx, &request.building_type_code).await?;
    let placement = PlaceBuildingRequest {
        map_code: request.map_code,
        building_type_code: request.building_type_code,
        tile_x: request.tile_x,
        tile_y: request.tile_y,
        tile_z: request.tile_z,
        expected_total_cost: String::new(),
    };
    validate_building_bounds(&placement, &map, &building)?;
    if placement.tile_x < 0 || placement.tile_y < 0 || placement.tile_z < 0 {
        return Err(ApiError::bad_request(
            "out_of_bounds",
            "building coordinates must be non-negative",
        ));
    }
    let occupied: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM economy_buildings WHERE map_id = $1 AND status <> 'demolished' \
         AND int4range(tile_x - 3, tile_x + footprint_width + 3, '[)') && int4range($2 - 3, $2 + $5 + 3, '[)') \
         AND int4range(tile_y - 3, tile_y + footprint_height + 3, '[)') && int4range($3 - 3, $3 + $6 + 3, '[)') \
         AND int4range(tile_z, tile_z + footprint_depth, '[)') && int4range($4, $4 + $7, '[)'))",
    )
    .bind(map.id).bind(placement.tile_x).bind(placement.tile_y).bind(placement.tile_z)
    .bind(building.footprint_width).bind(building.footprint_height).bind(building.footprint_depth)
    .fetch_one(&mut *tx).await.map_err(ApiError::from_sqlx)?;
    if occupied {
        return Err(ApiError::conflict(
            "building_overlap",
            "building footprint overlaps another building",
        ));
    }
    services::construction::estimate(&map, placement.tile_x, placement.tile_y, &building)
}

pub async fn place_building(
    db: &PgPool,
    uid: &str,
    request: PlaceBuildingRequest,
) -> Result<PlaceBuildingResponse, ApiError> {
    if request.map_code.trim().is_empty() || request.building_type_code.trim().is_empty() {
        return Err(ApiError::bad_request(
            "invalid_input",
            "map_code and building_type_code are required",
        ));
    }

    if request.tile_x < 0 || request.tile_y < 0 || request.tile_z < 0 {
        return Err(ApiError::bad_request(
            "out_of_bounds",
            "building coordinates must be non-negative",
        ));
    }

    services::player::ensure_player_rows(db, uid).await?;

    let mut tx = db.begin().await.map_err(ApiError::from_sqlx)?;
    db::settlement::settle_player_in_tx(&mut tx, uid).await?;

    let map = db::maps::load_map(&mut tx, &request.map_code).await?;
    let building_type =
        db::building_types::load_building_type(&mut tx, &request.building_type_code).await?;

    validate_building_bounds(&request, &map, &building_type)?;
    if request.expected_total_cost.is_empty() {
        return Err(ApiError::bad_request(
            "quote_required",
            "request a construction quote before placement",
        ));
    }
    let quote =
        services::construction::estimate(&map, request.tile_x, request.tile_y, &building_type)?;
    if quote.total_cost != request.expected_total_cost {
        return Err(ApiError::conflict(
            "stale_quote",
            "construction price changed; request a new quote",
        ));
    }
    db::users::charge_placement_mt(&mut tx, uid, &quote.total_cost).await?;

    let budget = db::build_budget::lock_build_budget(&mut tx, uid).await?;
    let required_cells = building_cells(
        building_type.footprint_width,
        building_type.footprint_height,
        building_type.footprint_depth,
    )?;

    if required_cells > budget.free_cells {
        return Err(ApiError::conflict(
            "insufficient_build_cells",
            "not enough free building cells",
        ));
    }

    let building_row =
        db::buildings::insert_building(&mut tx, uid, &map, &building_type, &request, Some(&quote))
            .await
            .map_err(map_place_building_error)?;

    tx.commit().await.map_err(ApiError::from_sqlx)?;

    Ok(PlaceBuildingResponse {
        building: Some(row_to_building(&building_row)),
        build_budget: Some(
            db::build_budget::load_build_budget(db, uid)
                .await?
                .into_proto(),
        ),
        ke_tse_balance: sqlx::query_scalar("SELECT balance::text FROM users WHERE uid = $1")
            .bind(uid)
            .fetch_one(db)
            .await
            .map_err(ApiError::from_sqlx)?,
    })
}

pub async fn set_workers(
    db: &PgPool,
    uid: &str,
    request: SetWorkersRequest,
) -> Result<SetWorkersResponse, ApiError> {
    if request.building_id.trim().is_empty() {
        return Err(ApiError::bad_request(
            "invalid_input",
            "building_id is required",
        ));
    }

    if request.assigned_workers < 0 {
        return Err(ApiError::bad_request(
            "invalid_input",
            "assigned_workers must be non-negative",
        ));
    }

    services::player::ensure_player_rows(db, uid).await?;

    let mut tx = db.begin().await.map_err(ApiError::from_sqlx)?;
    db::settlement::settle_player_in_tx(&mut tx, uid).await?;

    let context = db::buildings::load_worker_assignment_context(&mut tx, uid, &request.building_id)
        .await
        .map_err(map_uuid_error)?;

    let context =
        context.ok_or_else(|| ApiError::not_found("building_not_found", "building not found"))?;
    let max_workers = context.max_workers.ok_or_else(|| {
        ApiError::conflict(
            "worker_capacity_not_configured",
            "building worker capacity is not configured",
        )
    })?;

    if request.assigned_workers > max_workers {
        return Err(ApiError::conflict(
            "worker_capacity_exceeded",
            format!(
                "assigned_workers exceeds building capacity of {}",
                max_workers
            ),
        ));
    }

    let total_assigned_workers =
        context.other_assigned_workers + i64::from(request.assigned_workers);
    if total_assigned_workers > i64::from(context.total_population) {
        return Err(ApiError::conflict(
            "population_capacity_exceeded",
            "assigned workers exceed total population",
        ));
    }

    let row =
        db::buildings::set_workers(&mut tx, uid, &request.building_id, request.assigned_workers)
            .await
            .map_err(map_uuid_error)?;

    tx.commit().await.map_err(ApiError::from_sqlx)?;

    Ok(SetWorkersResponse {
        building: Some(row_to_building(&row)),
    })
}

pub async fn upgrade_building(
    db: &PgPool,
    uid: &str,
    request: UpgradeBuildingRequest,
) -> Result<UpgradeBuildingResponse, ApiError> {
    if request.building_id.trim().is_empty() {
        return Err(ApiError::bad_request(
            "invalid_input",
            "building_id is required",
        ));
    }

    services::player::ensure_player_rows(db, uid).await?;

    let mut tx = db.begin().await.map_err(ApiError::from_sqlx)?;
    db::settlement::settle_player_in_tx(&mut tx, uid).await?;

    let building_row = db::buildings::load_upgradable_building(&mut tx, uid, &request.building_id)
        .await
        .map_err(map_uuid_error)?
        .ok_or_else(|| {
            ApiError::conflict(
                "building_not_upgradable",
                "building was not found, is not active, or is already at max level",
            )
        })?;

    let building_type_id = building_row.get::<i64, _>("building_type_id");
    let current_level = building_row.get::<i32, _>("level");
    let cost_rules =
        db::buildings::load_upgrade_cost_rules(&mut tx, building_type_id, current_level)
            .await
            .map_err(ApiError::from_sqlx)?;

    if cost_rules.is_empty() {
        return Err(ApiError::conflict(
            "upgrade_not_available",
            "building upgrade is not available",
        ));
    }

    for cost_rule in cost_rules {
        if !cost_rule.amount_is_positive {
            return Err(ApiError::conflict(
                "invalid_upgrade_cost",
                "building upgrade cost must be positive",
            ));
        }

        let spent = match cost_rule.cost_kind.as_str() {
            "mt_currency" => db::buildings::spend_upgrade_mt(&mut tx, uid, &cost_rule.amount)
                .await
                .map_err(ApiError::from_sqlx)?,
            "exact_asset" => {
                let asset_family_id = cost_rule.asset_family_id.ok_or_else(|| {
                    ApiError::internal("upgrade exact asset cost is missing asset family")
                })?;
                let tier = cost_rule.min_tier.ok_or_else(|| {
                    ApiError::internal("upgrade exact asset cost is missing tier")
                })?;
                db::buildings::spend_upgrade_exact_asset(
                    &mut tx,
                    uid,
                    asset_family_id,
                    tier,
                    &cost_rule.amount,
                )
                .await
                .map_err(ApiError::from_sqlx)?
            }
            "category_points" => {
                let asset_family_id = cost_rule.asset_family_id.ok_or_else(|| {
                    ApiError::internal("upgrade category point cost is missing asset family")
                })?;
                let min_tier = cost_rule.min_tier.ok_or_else(|| {
                    ApiError::internal("upgrade category point cost is missing minimum tier")
                })?;
                db::buildings::spend_upgrade_category_points(
                    &mut tx,
                    uid,
                    asset_family_id,
                    min_tier,
                    &cost_rule.amount,
                )
                .await
                .map_err(ApiError::from_sqlx)?
            }
            _ => return Err(ApiError::internal("unknown upgrade cost kind")),
        };

        if !spent {
            return Err(ApiError::conflict(
                "not_enough_resources",
                "not enough resources to upgrade building",
            ));
        }
    }

    let upgraded_row = db::buildings::increment_building_level(&mut tx, uid, &request.building_id)
        .await
        .map_err(map_uuid_error)?
        .ok_or_else(|| {
            ApiError::conflict(
                "building_not_upgradable",
                "building was not found, is not active, or is already at max level",
            )
        })?;

    tx.commit().await.map_err(ApiError::from_sqlx)?;

    Ok(UpgradeBuildingResponse {
        building: Some(row_to_building(&upgraded_row)),
        balances: db::balances::load_balances(db, uid).await?,
    })
}

fn validate_building_bounds(
    request: &PlaceBuildingRequest,
    map: &MapRow,
    building_type: &BuildingTypeRow,
) -> Result<(), ApiError> {
    let x_end = request.tile_x as i64 + building_type.footprint_width as i64;
    let y_end = request.tile_y as i64 + building_type.footprint_height as i64;
    let z_end = request.tile_z as i64 + building_type.footprint_depth as i64;

    if x_end > map.width as i64 || y_end > map.height as i64 || z_end > map.depth as i64 {
        return Err(ApiError::bad_request(
            "out_of_bounds",
            "building footprint does not fit inside the map",
        ));
    }

    Ok(())
}

use crate::{db, pb, services};
use sqlx::PgPool;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to an initialized, disposable game database"]
async fn standalone_gameplay() {
    let url = std::env::var("TEST_DATABASE_URL").expect("set TEST_DATABASE_URL");
    let pool = PgPool::connect(&url).await.unwrap();
    db::migrate::migrate(&pool).await.unwrap();
    let uid = "epic-game-integration-test";

    let initial = services::snapshot::snapshot(&pool, uid, 0).await.unwrap();
    assert_eq!(initial.population.unwrap().total_population, 10);
    assert_eq!(initial.build_budget.unwrap().free_cells, 100);

    let quote = services::buildings::quote_building(
        &pool,
        uid,
        pb::QuoteBuildingRequest {
            map_code: "main_world".into(),
            building_type_code: "farm".into(),
            tile_x: 500_000,
            tile_y: 500_000,
            tile_z: 0,
        },
    )
    .await
    .unwrap();
    assert!(quote.total_cost.parse::<f64>().unwrap() >= 500.0);
    let placement = pb::PlaceBuildingRequest {
        map_code: "main_world".into(),
        building_type_code: "farm".into(),
        tile_x: 500_000,
        tile_y: 500_000,
        tile_z: 0,
        expected_total_cost: quote.total_cost.clone(),
    };
    let mut stale = placement.clone();
    stale.expected_total_cost = "1.00".into();
    assert!(services::buildings::place_building(&pool, uid, stale).await.is_err());
    let before: String = sqlx::query_scalar("SELECT balance::text FROM users WHERE uid = $1")
        .bind(uid).fetch_one(&pool).await.unwrap();
    assert_eq!(before, "2000");
    let placed = services::buildings::place_building(&pool, uid, placement.clone())
        .await
        .unwrap();
    let building = placed.building.unwrap();
    assert!(
        (building.planet_x.powi(2) + building.planet_y.powi(2) + building.planet_z.powi(2)).sqrt()
            > 3_389_500.0
    );
    assert_eq!(building.pad_elevation, quote.pad_elevation);
    assert_eq!(placed.build_budget.unwrap().free_cells, 75);
    assert!(
        services::buildings::place_building(&pool, uid, placement)
            .await
            .is_err()
    );

    // Retrying registration and a rejected placement must not reset/spend currency.
    db::player::ensure_player_rows(&pool, uid).await.unwrap();
    let balance: String = sqlx::query_scalar("SELECT balance::text FROM users WHERE uid = $1")
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        balance.parse::<f64>().unwrap(),
        2000.0 - quote.total_cost.parse::<f64>().unwrap()
    );

    let request = pb::SetWorkersRequest {
        building_id: building.id.clone(),
        assigned_workers: 11,
    };
    assert!(
        services::buildings::set_workers(&pool, uid, request)
            .await
            .is_err()
    );
    services::buildings::set_workers(
        &pool,
        uid,
        pb::SetWorkersRequest {
            building_id: building.id.clone(),
            assigned_workers: 10,
        },
    )
    .await
    .unwrap();

    sqlx::query("UPDATE player_economy_state SET last_settled_at = now() - interval '1 hour' WHERE uid = $1")
        .bind(uid).execute(&pool).await.unwrap();
    let settled = services::settlement::settle(&pool, uid).await.unwrap();
    assert_eq!(settled.settlement.unwrap().settled_hours, 1);
    let food = settled
        .balances
        .iter()
        .find(|b| b.asset_code == "food")
        .unwrap();
    assert_eq!(food.amount.parse::<f64>().unwrap(), 300.0);

    let upgraded = services::buildings::upgrade_building(
        &pool,
        uid,
        pb::UpgradeBuildingRequest {
            building_id: building.id,
        },
    )
    .await
    .unwrap();
    assert_eq!(upgraded.building.unwrap().level, 2);

    let population = services::population::increase_population(
        &pool,
        uid,
        pb::IncreasePopulationRequest { amount: 1 },
    )
    .await
    .unwrap();
    assert_eq!(population.population.unwrap().total_population, 11);
    assert_eq!(
        population.remaining_ke_tse_balance.parse::<f64>().unwrap(),
        1900.0 - quote.total_cost.parse::<f64>().unwrap()
    );
    assert_eq!(
        population
            .food_balance
            .unwrap()
            .amount
            .parse::<f64>()
            .unwrap(),
        100.0
    );
    assert!(
        services::population::increase_population(
            &pool,
            uid,
            pb::IncreasePopulationRequest { amount: 2 }
        )
        .await
        .is_err()
    );

    let final_state = services::snapshot::snapshot(&pool, uid, 0).await.unwrap();
    assert_eq!(final_state.population.unwrap().total_population, 11);
    assert_eq!(final_state.buildings.len(), 1);
    sqlx::query("DELETE FROM users WHERE uid = $1")
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

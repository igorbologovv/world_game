use crate::{
    constants::{
        SECTION_ALL, SECTION_BALANCES, SECTION_BUILD_BUDGET, SECTION_BUILDINGS, SECTION_POPULATION,
        SECTION_SETTLEMENT,
    },
    db,
    error::ApiError,
    pb::SnapshotResponse,
    services,
};
use sqlx::PgPool;

pub async fn snapshot(
    db: &PgPool,
    uid: &str,
    requested_section_mask: u32,
) -> Result<SnapshotResponse, ApiError> {
    let section_mask = if requested_section_mask == 0 {
        SECTION_ALL
    } else {
        requested_section_mask
    };

    services::player::ensure_player_rows(db, uid).await?;
    let settlement = db::settlement::settle_player(db, uid).await?;

    Ok(SnapshotResponse {
        settlement: include_section(section_mask, SECTION_SETTLEMENT).then_some(settlement),
        population: if include_section(section_mask, SECTION_POPULATION) {
            Some(db::population::load_population(db, uid).await?)
        } else {
            None
        },
        balances: if include_section(section_mask, SECTION_BALANCES) {
            db::balances::load_balances(db, uid).await?
        } else {
            Vec::new()
        },
        buildings: if include_section(section_mask, SECTION_BUILDINGS) {
            db::buildings::load_buildings(db, uid).await?
        } else {
            Vec::new()
        },
        build_budget: if include_section(section_mask, SECTION_BUILD_BUDGET) {
            Some(
                db::build_budget::load_build_budget(db, uid)
                    .await?
                    .into_proto(),
            )
        } else {
            None
        },
    })
}

fn include_section(section_mask: u32, section: u32) -> bool {
    section_mask & section != 0
}

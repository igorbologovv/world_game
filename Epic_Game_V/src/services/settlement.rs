use crate::{db, error::ApiError, pb::SettleResponse, services};
use sqlx::PgPool;

pub async fn settle(db: &PgPool, uid: &str) -> Result<SettleResponse, ApiError> {
    services::player::ensure_player_rows(db, uid).await?;

    Ok(SettleResponse {
        settlement: Some(db::settlement::settle_player(db, uid).await?),
        balances: db::balances::load_balances(db, uid).await?,
    })
}

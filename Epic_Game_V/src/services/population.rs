use crate::{
    db,
    error::ApiError,
    pb::{IncreasePopulationRequest, IncreasePopulationResponse},
    services,
};
use sqlx::PgPool;

pub async fn increase_population(
    db: &PgPool,
    uid: &str,
    request: IncreasePopulationRequest,
) -> Result<IncreasePopulationResponse, ApiError> {
    let amount = i32::try_from(request.amount)
        .map_err(|_| ApiError::bad_request("invalid_amount", "amount must be positive"))?;

    services::player::ensure_player_rows(db, uid).await?;
    db::population::increase_player_population(db, uid, amount).await
}

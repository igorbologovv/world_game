use crate::{
    auth::authenticated_uid,
    error::ApiError,
    pb::IncreasePopulationRequest,
    protocol::{decode_proto, proto_response},
    services,
    state::AppState,
};
use axum::{body::Bytes, extract::State, http::HeaderMap, http::StatusCode, response::Response};

pub async fn increase_population(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let uid = authenticated_uid(&headers).await?;
    let request = decode_proto::<IncreasePopulationRequest>(&body)?;
    let response = services::population::increase_population(&state.db, &uid, request).await?;

    Ok(proto_response(StatusCode::OK, &response))
}

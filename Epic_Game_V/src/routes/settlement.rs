use crate::{
    auth::authenticated_uid,
    error::ApiError,
    pb::SettleRequest,
    protocol::{decode_proto, proto_response},
    services,
    state::AppState,
};
use axum::{body::Bytes, extract::State, http::HeaderMap, http::StatusCode, response::Response};

pub async fn settle(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let uid = authenticated_uid(&headers).await?;
    let _request = decode_proto::<SettleRequest>(&body)?;
    let response = services::settlement::settle(&state.db, &uid).await?;

    Ok(proto_response(StatusCode::OK, &response))
}

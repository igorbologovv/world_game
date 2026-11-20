use crate::{
    auth::authenticated_uid,
    error::ApiError,
    pb::SnapshotRequest,
    protocol::{decode_proto, proto_response},
    services,
    state::AppState,
};
use axum::{body::Bytes, extract::State, http::HeaderMap, http::StatusCode, response::Response};

pub async fn snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let uid = authenticated_uid(&headers).await?;
    let request = decode_proto::<SnapshotRequest>(&body)?;
    let response = services::snapshot::snapshot(&state.db, &uid, request.section_mask).await?;

    Ok(proto_response(StatusCode::OK, &response))
}

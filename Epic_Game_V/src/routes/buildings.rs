use crate::{
    auth::authenticated_uid,
    error::ApiError,
    pb::{PlaceBuildingRequest, QuoteBuildingRequest, SetWorkersRequest, UpgradeBuildingRequest},
    protocol::{decode_proto, proto_response},
    services,
    state::AppState,
};
use axum::{body::Bytes, extract::State, http::HeaderMap, http::StatusCode, response::Response};

pub async fn quote_building(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let uid = authenticated_uid(&headers).await?;
    let request = decode_proto::<QuoteBuildingRequest>(&body)?;
    let response = services::buildings::quote_building(&state.db, &uid, request).await?;
    Ok(proto_response(StatusCode::OK, &response))
}

pub async fn place_building(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let uid = authenticated_uid(&headers).await?;
    let request = decode_proto::<PlaceBuildingRequest>(&body)?;
    let response = services::buildings::place_building(&state.db, &uid, request).await?;

    Ok(proto_response(StatusCode::OK, &response))
}

pub async fn set_workers(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let uid = authenticated_uid(&headers).await?;
    let request = decode_proto::<SetWorkersRequest>(&body)?;
    let response = services::buildings::set_workers(&state.db, &uid, request).await?;

    Ok(proto_response(StatusCode::OK, &response))
}

pub async fn upgrade_building(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let uid = authenticated_uid(&headers).await?;
    let request = decode_proto::<UpgradeBuildingRequest>(&body)?;
    let response = services::buildings::upgrade_building(&state.db, &uid, request).await?;

    Ok(proto_response(StatusCode::OK, &response))
}

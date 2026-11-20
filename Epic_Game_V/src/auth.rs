use crate::error::ApiError;
use crate::firebase_auth::{AuthError, authenticate_bearer_authorization};
use axum::http::{HeaderMap, header::AUTHORIZATION};

pub async fn authenticated_uid(headers: &HeaderMap) -> Result<String, ApiError> {
    let auth_header = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    authenticate_bearer_authorization(auth_header)
        .await
        .map_err(map_auth_error)
}

fn map_auth_error(error: AuthError) -> ApiError {
    match error {
        AuthError::MissingBearerToken | AuthError::InvalidBearerToken => {
            ApiError::unauthorized("missing authorization bearer token")
        }
        AuthError::TokenVerification(_) => ApiError::unauthorized("invalid or expired token"),
    }
}

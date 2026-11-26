use crate::{pb::ErrorResponse, protocol::proto_response};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let response = ErrorResponse {
            code: self.code.to_string(),
            message: self.message,
        };
        proto_response(self.status, &response)
    }
}

impl ApiError {
    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }

    pub fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code,
            message: message.into(),
        }
    }

    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
            message: message.into(),
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "db_error",
            message: message.into(),
        }
    }

    pub fn from_sqlx(error: sqlx::Error) -> Self {
        if let Some(db_error) = error.as_database_error() {
            if db_error.code().as_deref() == Some("23503") {
                return Self::not_found("user_not_found", "user not found");
            }
        }

        tracing::error!(%error, "economy database error");
        Self::internal("unexpected database error")
    }
}

pub fn map_uuid_error(error: sqlx::Error) -> ApiError {
    if let Some(db_error) = error.as_database_error() {
        if db_error.code().as_deref() == Some("22P02") {
            return ApiError::bad_request("invalid_building_id", "building_id must be a UUID");
        }
    }

    ApiError::from_sqlx(error)
}

pub fn map_place_building_error(error: sqlx::Error) -> ApiError {
    if let Some(db_error) = error.as_database_error() {
        if db_error.code().as_deref() == Some("23P01") {
            return ApiError::conflict(
                "building_overlap",
                "building footprint overlaps another building",
            );
        }
    }

    ApiError::from_sqlx(error)
}

pub fn map_population_increase_error(error: sqlx::Error) -> ApiError {
    if let Some(db_error) = error.as_database_error() {
        match db_error.code().as_deref() {
            Some("22023") => {
                return ApiError::bad_request("invalid_amount", "amount must be positive");
            }
            Some("P1002") => {
                return ApiError::conflict(
                    "insufficient_ke_tse",
                    "not enough KE-TSE to increase population",
                );
            }
            Some("P1003") => {
                return ApiError::conflict(
                    "insufficient_food",
                    "not enough food to increase population",
                );
            }
            _ => {}
        }
    }

    ApiError::from_sqlx(error)
}

use crate::state::AppState;
use axum::{extract::State, http::StatusCode};

pub async fn health() -> &'static str {
    "OK"
}

pub async fn db_health(State(state): State<AppState>) -> StatusCode {
    match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => StatusCode::OK,
        Err(error) => {
            tracing::error!(%error, "economy db health check failed");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

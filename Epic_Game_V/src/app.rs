use crate::{routes, state::AppState};
use axum::{
    Router,
    routing::{get, post},
};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(routes::health::health))
        .route("/internal/health/db", get(routes::health::db_health))
        .route("/economy/v1/snapshot", post(routes::snapshot::snapshot))
        .route("/economy/v1/settle", post(routes::settlement::settle))
        .route(
            "/economy/v1/population/increase",
            post(routes::population::increase_population),
        )
        .route(
            "/economy/v1/buildings/place",
            post(routes::buildings::place_building),
        )
        .route(
            "/economy/v1/buildings/set-workers",
            post(routes::buildings::set_workers),
        )
        .route(
            "/economy/v1/buildings/upgrade",
            post(routes::buildings::upgrade_building),
        )
        .with_state(state)
}

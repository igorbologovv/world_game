mod app;
mod auth;
mod constants;
mod db;
mod error;
mod firebase_auth;
mod pb;
mod protocol;
mod routes;
mod services;
mod state;

#[cfg(test)]
mod integration_tests;

use sqlx::postgres::PgPoolOptions;
use state::AppState;
use std::{env, net::SocketAddr};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let db = PgPoolOptions::new()
        .max_connections(10)
        .connect(&env::var("DATABASE_URL")?)
        .await?;

    let app = app::router(AppState::new(db));

    let addr: SocketAddr = env::var("ECONOMY_SERVICE_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:3010".to_string())
        .parse()?;

    tracing::info!("starting economy service on {addr}");
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;

    Ok(())
}

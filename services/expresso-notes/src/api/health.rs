//! Liveness + readiness probes.

use axum::{http::StatusCode, routing::get, Router};

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/ready", get(ready))
}

async fn ready(axum::extract::State(state): axum::extract::State<AppState>) -> StatusCode {
    match state.db_or_unavailable() {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

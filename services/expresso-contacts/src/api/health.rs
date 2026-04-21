//! Health/readiness endpoints

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde_json::{json, Value};

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/ready",  get(ready))
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok", "service": "expresso-contacts"}))
}

async fn ready(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let ready = match state.db() {
        Some(db) => sqlx::query("SELECT 1").execute(db).await.is_ok(),
        None => false,
    };

    let status = if ready { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (status, Json(json!({"ready": ready})))
}

//! Health/readiness endpoints

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde_json::{json, Value};

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/ready",  get(ready))
        .route("/readyz", get(readyz))
}

async fn readyz(State(state): State<AppState>) -> impl axum::response::IntoResponse {
    use expresso_core::health::{ReadinessCheck, db_check};
    let mut checks: Vec<ReadinessCheck> = Vec::new();
    if let Some(db) = state.db() {
        checks.push(ReadinessCheck { name: "db", required: true, run: db_check(db.clone()) });
    }
    let (code, report) = expresso_core::health::run(&checks).await;
    (code, axum::Json(report))
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

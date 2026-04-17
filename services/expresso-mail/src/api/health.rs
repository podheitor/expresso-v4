use axum::{Router, routing::get, Json};
use serde_json::json;

pub fn routes() -> Router<crate::state::AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/ready",  get(ready))
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok", "service": "expresso-mail"}))
}

async fn ready(
    axum::extract::State(state): axum::extract::State<crate::state::AppState>,
) -> Json<serde_json::Value> {
    // Check DB reachability
    let db_ok = sqlx::query("SELECT 1")
        .execute(state.db())
        .await
        .is_ok();

    let status = if db_ok { "ready" } else { "degraded" };
    Json(json!({"status": status, "db": db_ok}))
}

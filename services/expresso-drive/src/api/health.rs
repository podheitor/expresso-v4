use axum::{Json, Router, routing::get};
use serde_json::{json, Value};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/ready",  get(ready))
}

async fn health() -> Json<Value> { Json(json!({"service":"expresso-drive","status":"ok"})) }
async fn ready()  -> Json<Value> { Json(json!({"ready": true})) }

//! expresso-search — full-text search service backed by Tantivy.
//!
//! Endpoints:
//!   GET  /health                     → service health
//!   GET  /ready                      → readiness
//!   POST /api/v1/index               → index document
//!   GET  /api/v1/search?q=&tenant_id=  → search
//!   DELETE /api/v1/index/:id         → remove document

mod api;
mod index_store;

use std::{env, net::SocketAddr, path::PathBuf};

use axum::{
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tracing::info;

use index_store::IndexStore;

const SERVICE: &str = "expresso-search";
const DEFAULT_PORT: u16 = 8007;

async fn health() -> Json<Value> {
    Json(json!({"service": SERVICE, "status": "ok"}))
}

async fn ready() -> Json<Value> {
    Json(json!({"ready": true}))
}

fn resolve_addr() -> anyhow::Result<SocketAddr> {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    Ok(format!("{}:{}", host, port).parse()?)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let data_dir = env::var("SEARCH_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/var/lib/expresso/search"));

    let store = IndexStore::open(&data_dir)?;

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/api/v1/index", post(api::index_doc))
        .route("/api/v1/index/{id}", delete(api::delete_doc))
        .route("/api/v1/search", get(api::search))
        .with_state(store);

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(service = SERVICE, %addr, "listening");
    axum::serve(listener, app).await?;

    Ok(())
}

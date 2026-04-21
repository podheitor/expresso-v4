//! expresso-auth service entrypoint

use std::{env, net::SocketAddr};

use axum::{routing::get, Json, Router};
use serde_json::{json, Value};
use tracing::info;

const SERVICE: &str = "expresso-auth";
const DEFAULT_PORT: u16 = 8100;

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

    let addr = format!("{}:{}", host, port)
        .parse::<SocketAddr>()
        .map_err(|e| anyhow::anyhow!("invalid bind address: {}", e))?;

    Ok(addr)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready));
    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(service = SERVICE, %addr, "listening");

    axum::serve(listener, app).await?;

    Ok(())
}

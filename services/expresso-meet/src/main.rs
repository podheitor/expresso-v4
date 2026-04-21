//! expresso-meet service entrypoint — minimal /health + /ready stub.
//!
//! Future: BFF for Jitsi/LiveKit room provisioning + tenant auth bridge.
//! Conventions match sibling services (`init_tracing`, `SERVER__HOST/PORT`,
//! `TELEMETRY__*`) so deploy wiring stays uniform.

use std::{env, net::SocketAddr};

use axum::{routing::get, Json, Router};
use serde_json::{json, Value};
use tracing::info;

use expresso_core::{config::TelemetryConfig, init_tracing};

const SERVICE: &str = "expresso-meet";
const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 8011;
const DEFAULT_OTLP_ENDPOINT: &str = "http://localhost:4317";
const DEFAULT_LOG_FILTER: &str = "info";

fn env_string(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.trim().is_empty())
}
fn env_u16(key: &str, d: u16) -> u16 {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn env_bool(key: &str, d: bool) -> bool {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

async fn health() -> Json<Value> {
    Json(json!({"service": SERVICE, "status": "ok"}))
}

async fn ready() -> Json<Value> {
    Json(json!({"ready": true}))
}

fn resolve_addr() -> anyhow::Result<SocketAddr> {
    let host = env_string("SERVER__HOST").unwrap_or_else(|| DEFAULT_HOST.to_string());
    let port = env_u16("SERVER__PORT", DEFAULT_PORT);
    format!("{}:{}", host, port)
        .parse::<SocketAddr>()
        .map_err(|e| anyhow::anyhow!("invalid bind address: {}", e))
}

fn resolve_telemetry() -> TelemetryConfig {
    TelemetryConfig {
        otlp_endpoint: env_string("TELEMETRY__OTLP_ENDPOINT")
            .unwrap_or_else(|| DEFAULT_OTLP_ENDPOINT.to_string()),
        log_json: env_bool("TELEMETRY__LOG_JSON", false),
        log_filter: env_string("TELEMETRY__LOG_FILTER")
            .unwrap_or_else(|| DEFAULT_LOG_FILTER.to_string()),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = resolve_telemetry();
    init_tracing(&telemetry);

    info!(version = env!("CARGO_PKG_VERSION"), "{SERVICE} starting");

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready));

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(service = SERVICE, %addr, "HTTP listening");
    axum::serve(listener, app).await?;
    Ok(())
}

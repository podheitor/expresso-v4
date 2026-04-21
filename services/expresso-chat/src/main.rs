//! expresso-chat service entrypoint.
//!
//! Role: BFF between the Expresso frontend and a Synapse (Matrix) homeserver.
//! REST endpoints mirror the Expresso UX model (channels/teams/messages) and
//! translate to Matrix CS API calls. Tenant metadata lives in Postgres.

mod api;
mod domain;
mod error;
mod matrix;
mod state;

use std::{env, net::SocketAddr};

use tracing::{info, warn};

use expresso_core::{create_db_pool, init_tracing};
use expresso_core::config::{DatabaseConfig, TelemetryConfig};
use state::AppState;

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 8004;
const DEFAULT_DB_MAX_CONNECTIONS: u32 = 20;
const DEFAULT_DB_MIN_CONNECTIONS: u32 = 2;
const DEFAULT_DB_ACQUIRE_TIMEOUT_SECS: u64 = 5;
const DEFAULT_OTLP_ENDPOINT: &str = "http://localhost:4317";
const DEFAULT_LOG_FILTER: &str = "info";

fn env_string(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.trim().is_empty())
}
fn env_u16(key: &str, d: u16) -> u16 { env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }
fn env_u32(key: &str, d: u32) -> u32 { env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }
fn env_u64(key: &str, d: u64) -> u64 { env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }
fn env_bool(key: &str, d: bool) -> bool { env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }

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

fn resolve_database_config() -> Option<DatabaseConfig> {
    let url = env_string("DATABASE__URL")?;
    Some(DatabaseConfig {
        url,
        max_connections: env_u32("DATABASE__MAX_CONNECTIONS", DEFAULT_DB_MAX_CONNECTIONS),
        min_connections: env_u32("DATABASE__MIN_CONNECTIONS", DEFAULT_DB_MIN_CONNECTIONS),
        acquire_timeout_secs: env_u64("DATABASE__ACQUIRE_TIMEOUT_SECS", DEFAULT_DB_ACQUIRE_TIMEOUT_SECS),
    })
}

fn resolve_matrix_config() -> Option<matrix::MatrixConfig> {
    let hs_url        = env_string("MATRIX__HS_URL")?;
    let server_name   = env_string("MATRIX__SERVER_NAME")?;
    let as_token      = env_string("MATRIX__AS_TOKEN");
    let admin_token   = env_string("MATRIX__ADMIN_TOKEN");
    Some(matrix::MatrixConfig { hs_url, server_name, as_token, admin_token })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = resolve_telemetry();
    init_tracing(&telemetry);

    info!(version = env!("CARGO_PKG_VERSION"), "expresso-chat starting");

    let db = match resolve_database_config() {
        Some(cfg) => match create_db_pool(&cfg).await {
            Ok(pool) => Some(pool),
            Err(e) => { warn!(error = %e, "database unavailable; readiness degraded"); None }
        },
        None => { warn!("database config missing; readiness degraded"); None }
    };

    let matrix = resolve_matrix_config().map(matrix::MatrixClient::new);
    if matrix.is_none() {
        warn!("matrix config missing (MATRIX__HS_URL + MATRIX__SERVER_NAME); chat routes degraded");
    }

    let http_addr = resolve_addr()?;
    let state = AppState::new(db, matrix);
    let app = api::router(state);
    let listener = tokio::net::TcpListener::bind(http_addr).await?;

    info!(addr = %http_addr, "HTTP API listening");
    axum::serve(listener, app).await?;
    Ok(())
}

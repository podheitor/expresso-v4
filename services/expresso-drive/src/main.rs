//! expresso-drive service entrypoint

mod api;
mod domain;
mod error;
mod state;

use std::{env, net::SocketAddr, path::PathBuf};

use tracing::{info, warn};

use expresso_core::{create_db_pool, init_tracing};
use expresso_core::config::{DatabaseConfig, TelemetryConfig};
use state::AppState;

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 8004;
const DEFAULT_DATA_ROOT: &str = "/var/lib/expresso/drive";
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
    let host = env_string("SERVER__HOST").unwrap_or_else(|| DEFAULT_HOST.into());
    let port = env_u16("SERVER__PORT", DEFAULT_PORT);
    format!("{host}:{port}").parse().map_err(|e| anyhow::anyhow!("invalid bind: {e}"))
}

fn resolve_telemetry() -> TelemetryConfig {
    TelemetryConfig {
        otlp_endpoint: env_string("TELEMETRY__OTLP_ENDPOINT").unwrap_or_else(|| DEFAULT_OTLP_ENDPOINT.into()),
        log_json:      env_bool("TELEMETRY__LOG_JSON", false),
        log_filter:    env_string("TELEMETRY__LOG_FILTER").unwrap_or_else(|| DEFAULT_LOG_FILTER.into()),
    }
}

fn resolve_db() -> Option<DatabaseConfig> {
    Some(DatabaseConfig {
        url:                  env_string("DATABASE__URL")?,
        max_connections:      env_u32("DATABASE__MAX_CONNECTIONS", 20),
        min_connections:      env_u32("DATABASE__MIN_CONNECTIONS", 2),
        acquire_timeout_secs: env_u64("DATABASE__ACQUIRE_TIMEOUT_SECS", 5),
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let tel = resolve_telemetry();
    init_tracing(&tel);

    info!(version = env!("CARGO_PKG_VERSION"), "expresso-drive starting");

    let db = match resolve_db() {
        Some(cfg) => match create_db_pool(&cfg).await {
            Ok(p)  => Some(p),
            Err(e) => { warn!(error=%e, "db unavailable"); None }
        },
        None => { warn!("db config missing"); None }
    };

    let data_root = PathBuf::from(
        env_string("DRIVE__DATA_ROOT").unwrap_or_else(|| DEFAULT_DATA_ROOT.into())
    );
    if let Err(e) = tokio::fs::create_dir_all(&data_root).await {
        warn!(error=%e, path=%data_root.display(), "cannot create data_root");
    }

    let addr  = resolve_addr()?;
    let state = AppState::new(db, data_root);
    let app   = api::router(state);
    let lst   = tokio::net::TcpListener::bind(addr).await?;

    info!(%addr, "HTTP API listening");
    axum::serve(lst, app).await?;
    Ok(())
}

//! expresso-meet service entrypoint — Jitsi Meet BFF.
//!
//! Role: mint JWT tokens for Jitsi (HS256 with shared app_secret), register
//! meetings + participant ACL in Postgres, expose REST for the Expresso UI.
//! Rooms themselves are ephemeral on Jitsi — we only control access.

mod api;
mod domain;
mod error;
mod jitsi;
mod state;

use std::{env, net::SocketAddr, sync::Arc};

use tracing::{error, info, warn};

use expresso_auth_client::{OidcConfig, OidcValidator};

use expresso_core::{config::{DatabaseConfig, TelemetryConfig}, create_db_pool, init_tracing};
use state::AppState;

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 8011;
const DEFAULT_DB_MAX_CONNECTIONS: u32 = 10;
const DEFAULT_DB_MIN_CONNECTIONS: u32 = 1;
const DEFAULT_DB_ACQUIRE_TIMEOUT_SECS: u64 = 5;
const DEFAULT_OTLP_ENDPOINT: &str = "http://localhost:4317";
const DEFAULT_LOG_FILTER: &str = "info";
const DEFAULT_JWT_TTL_SECS: i64 = 3600;
const DEFAULT_ROOM_PREFIX: &str = "exp-";

fn env_string(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.trim().is_empty())
}
fn env_u16(key: &str, d: u16) -> u16 { env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }
fn env_u32(key: &str, d: u32) -> u32 { env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }
fn env_u64(key: &str, d: u64) -> u64 { env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }
fn env_i64(key: &str, d: i64) -> i64 { env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }
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

fn resolve_jitsi_config() -> Option<jitsi::JitsiConfig> {
    let app_id     = env_string("JITSI__APP_ID")?;
    let app_secret = env_string("JITSI__APP_SECRET")?;
    let domain     = env_string("JITSI__DOMAIN")?;
    Some(jitsi::JitsiConfig {
        app_id,
        app_secret,
        domain,
        jwt_ttl:     env_i64("JITSI__JWT_TTL_SECS", DEFAULT_JWT_TTL_SECS),
        room_prefix: env_string("JITSI__ROOM_PREFIX")
            .unwrap_or_else(|| DEFAULT_ROOM_PREFIX.to_string()),
    })
}


/// Build an `OidcValidator` from env. Returns `None` when the issuer/audience
/// pair is unset (service runs in dev-header-auth mode with a loud warning).
async fn resolve_oidc() -> Option<Arc<OidcValidator>> {
    let issuer   = env_string("AUTH__OIDC_ISSUER")?;
    let audience = env_string("AUTH__OIDC_AUDIENCE")?;
    let cfg = OidcConfig::new(issuer.clone(), audience.clone());
    match OidcValidator::new(cfg).await {
        Ok(v)  => { info!(%issuer, %audience, "OIDC validator ready"); Some(Arc::new(v)) }
        Err(e) => { error!(error = %e, %issuer, "OIDC validator init failed — falling back to header auth (INSECURE)"); None }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = resolve_telemetry();
    init_tracing(&telemetry);

    info!(version = env!("CARGO_PKG_VERSION"), "expresso-meet starting");

    let db = match resolve_database_config() {
        Some(cfg) => match create_db_pool(&cfg).await {
            Ok(pool) => Some(pool),
            Err(e) => { warn!(error = %e, "database unavailable; readiness degraded"); None }
        },
        None => { warn!("database config missing; readiness degraded"); None }
    };

    let jitsi = resolve_jitsi_config().map(jitsi::Jitsi::new);
    if jitsi.is_none() {
        warn!("jitsi config missing (JITSI__APP_ID + JITSI__APP_SECRET + JITSI__DOMAIN); meeting routes degraded");
    }

    let oidc = resolve_oidc().await;
    if oidc.is_none() {
        warn!("AUTH__OIDC_ISSUER / AUTH__OIDC_AUDIENCE unset — auth in DEV header-mode (INSECURE)");
    }
    let http_addr = resolve_addr()?;
    let state = AppState::new(db, jitsi);
    let app = api::router(state, oidc);
    let listener = tokio::net::TcpListener::bind(http_addr).await?;

    info!(addr = %http_addr, "HTTP API listening");
    axum::serve(listener, app).await?;
    Ok(())
}

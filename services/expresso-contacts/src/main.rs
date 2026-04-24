//! expresso-contacts service entrypoint

mod api;
mod carddav;
mod domain;
mod error;
mod events;
mod state;

use std::{env, net::SocketAddr, sync::Arc};

use tracing::{info, warn};

use expresso_core::{create_db_pool, init_tracing};
use expresso_core::config::{DatabaseConfig, TelemetryConfig};
use state::AppState;

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 8003;
const DEFAULT_DB_MAX_CONNECTIONS: u32 = 20;
const DEFAULT_DB_MIN_CONNECTIONS: u32 = 2;
const DEFAULT_DB_ACQUIRE_TIMEOUT_SECS: u64 = 5;
const DEFAULT_OTLP_ENDPOINT: &str = "http://localhost:4317";
const DEFAULT_LOG_FILTER: &str = "info";

fn env_string(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn env_u16(key: &str, default: u16) -> u16 {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}
fn env_u32(key: &str, default: u32) -> u32 {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}
fn env_i32(key: &str, default: i32) -> i32 {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}
fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}
fn env_bool(key: &str, default: bool) -> bool {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
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

fn resolve_database_config() -> Option<DatabaseConfig> {
    let url = env_string("DATABASE__URL")?;
    Some(DatabaseConfig {
        url,
        max_connections: env_u32("DATABASE__MAX_CONNECTIONS", DEFAULT_DB_MAX_CONNECTIONS),
        min_connections: env_u32("DATABASE__MIN_CONNECTIONS", DEFAULT_DB_MIN_CONNECTIONS),
        acquire_timeout_secs: env_u64("DATABASE__ACQUIRE_TIMEOUT_SECS", DEFAULT_DB_ACQUIRE_TIMEOUT_SECS),
    })
}


fn resolve_multi_realm() -> (
    Option<Arc<expresso_auth_client::MultiRealmValidator>>,
    Option<Arc<expresso_auth_client::TenantResolver>>,
) {
    let tpl = match env_string("AUTH__OIDC_ISSUER_TEMPLATE") { Some(v) => v, None => return (None, None) };
    let audience = match env_string("AUTH__OIDC_AUDIENCE")   { Some(v) => v, None => return (None, None) };
    let resolver = expresso_auth_client::TenantResolver::from_env("AUTH__TENANT_HOSTS");
    if resolver.is_empty() {
        warn!("AUTH__TENANT_HOSTS empty — multi-realm disabled");
        return (None, None);
    }
    match expresso_auth_client::MultiRealmValidator::new(tpl.clone(), audience.clone()) {
        Ok(m)  => {
            info!(template = %tpl, hosts = resolver.len(), "multi-realm validator ready");
            (Some(Arc::new(m)), Some(Arc::new(resolver)))
        }
        Err(e) => { tracing::error!(error = %e, "multi-realm init failed"); (None, None) }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = resolve_telemetry();
    init_tracing(&telemetry);

    info!(version = env!("CARGO_PKG_VERSION"), "expresso-contacts starting");

    let db = match resolve_database_config() {
        Some(cfg) => match create_db_pool(&cfg).await {
            Ok(pool) => Some(pool),
            Err(e) => { warn!(error = %e, "database unavailable; readiness degraded"); None }
        },
        None => { warn!("database config missing; readiness degraded"); None }
    };

    let http_addr = resolve_addr()?;
    let kc_basic = expresso_auth_client::KcBasicConfig::from_env_prefix("CARDDAV_KC").map(expresso_auth_client::KcBasicAuthenticator::new);
    if kc_basic.is_some() { tracing::info!("CardDAV Keycloak Basic auth enabled"); }
    if let Some(pool) = db.clone() {
        let retention = env_i32("DAV_TOMBSTONE_RETENTION_DAYS", domain::tombstone_gc::DEFAULT_RETENTION_DAYS);
        let every = env_u64("DAV_TOMBSTONE_GC_INTERVAL_HOURS", domain::tombstone_gc::DEFAULT_INTERVAL_HOURS);
        info!(retention_days = retention, interval_hours = every, "spawning tombstone GC");
        domain::tombstone_gc::spawn(pool, retention, every);
    }
    // Sprint #23: opt-in NATS JetStream via NATS_URL.
    let bus = match std::env::var("NATS_URL").ok().filter(|v| !v.trim().is_empty()) {
        Some(url) => match crate::events::ContactsEventBus::new_with_nats(&url).await {
            Ok(b) => { tracing::info!(nats_url=%url, "contacts EventBus with NATS enabled"); b }
            Err(e) => { tracing::warn!(error=%e, "NATS init failed; noop bus"); crate::events::ContactsEventBus::noop() }
        },
        None => crate::events::ContactsEventBus::noop(),
    };
    let state = AppState::new(db, kc_basic, bus);
    // Per-tenant rate limiter (shared core; see expresso_core::ratelimit).
    let rate_cfg = expresso_core::ratelimit::RateLimitConfig::from_env();
    info!(rps = rate_cfg.rps, burst = rate_cfg.burst, "rate limiter armed");
    let rate_limiter = expresso_core::ratelimit::RateLimiter::new(rate_cfg);
    {
        let rl = rate_limiter.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                rl.gc();
            }
        });
    }
    let (multi, resolver) = resolve_multi_realm();
    let mut app = api::router(state)
        .layer(axum::middleware::from_fn(expresso_core::ratelimit::layer))
        .layer(axum::extract::Extension(rate_limiter));
    if let Some(m) = multi    { app = app.layer(axum::extract::Extension(m)); }
    if let Some(r) = resolver { app = app.layer(axum::extract::Extension(r)); }
    let listener = tokio::net::TcpListener::bind(http_addr).await?;

    info!(addr = %http_addr, "HTTP API listening");
    axum::serve(listener, app).await?;
    Ok(())
}

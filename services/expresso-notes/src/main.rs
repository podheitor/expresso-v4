//! expresso-notes service entrypoint — personal notes/memos CRUD.
//!
//! Tenant-scoped notes over Postgres (RLS), REST API for the Expresso UI. Notes
//! are full-text indexed in expresso-search (`kind = "note"`) when SEARCH__URL
//! is configured.

mod api;
mod domain;
mod error;
mod state;

use std::{env, net::SocketAddr, sync::Arc};

use tracing::{error, info, warn};

use expresso_auth_client::{MultiRealmValidator, OidcConfig, OidcValidator, TenantResolver};
use expresso_core::{
    config::{DatabaseConfig, TelemetryConfig},
    create_db_pool, init_tracing,
};
use state::AppState;

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 8012;
const DEFAULT_DB_MAX_CONNECTIONS: u32 = 10;
const DEFAULT_DB_MIN_CONNECTIONS: u32 = 1;
const DEFAULT_DB_ACQUIRE_TIMEOUT_SECS: u64 = 5;
const DEFAULT_OTLP_ENDPOINT: &str = "http://localhost:4317";
const DEFAULT_LOG_FILTER: &str = "info";

fn env_string(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.trim().is_empty())
}
fn env_u16(key: &str, d: u16) -> u16 {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn env_u32(key: &str, d: u32) -> u32 {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn env_u64(key: &str, d: u64) -> u64 {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn env_bool(key: &str, d: bool) -> bool {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
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
        acquire_timeout_secs: env_u64(
            "DATABASE__ACQUIRE_TIMEOUT_SECS",
            DEFAULT_DB_ACQUIRE_TIMEOUT_SECS,
        ),
    })
}

/// Build an `OidcValidator` from env. Returns `None` when issuer/audience are
/// unset (service runs in dev-header-auth mode with a loud warning).
async fn resolve_oidc() -> Option<Arc<OidcValidator>> {
    let issuer = env_string("AUTH__OIDC_ISSUER")?;
    let audience = env_string("AUTH__OIDC_AUDIENCE")?;
    let cfg = OidcConfig::new(issuer.clone(), audience.clone());
    match OidcValidator::new(cfg).await {
        Ok(v) => {
            info!(%issuer, %audience, "OIDC validator ready");
            Some(Arc::new(v))
        }
        Err(e) => {
            error!(error = %e, %issuer, "OIDC validator init failed — falling back to header auth (INSECURE)");
            None
        }
    }
}

/// Build the PAT resolver from `AUTH__URL`. Absent → None (PAT path off, JWT
/// only). Opt-in per deployment.
fn resolve_pat() -> Option<Arc<expresso_auth_client::PatResolver>> {
    let auth_url = env_string("AUTH__URL")?;
    info!(%auth_url, "PAT resolver enabled");
    Some(Arc::new(expresso_auth_client::PatResolver::new(
        auth_url,
        reqwest::Client::new(),
    )))
}

fn resolve_multi_realm() -> (
    Option<Arc<MultiRealmValidator>>,
    Option<Arc<TenantResolver>>,
) {
    let tpl = match env_string("AUTH__OIDC_ISSUER_TEMPLATE") {
        Some(v) => v,
        None => return (None, None),
    };
    let audience = match env_string("AUTH__OIDC_AUDIENCE") {
        Some(v) => v,
        None => return (None, None),
    };
    let resolver = TenantResolver::from_env("AUTH__TENANT_HOSTS");
    if resolver.is_empty() {
        warn!("AUTH__TENANT_HOSTS empty — multi-realm disabled");
        return (None, None);
    }
    match MultiRealmValidator::new(tpl.clone(), audience.clone()) {
        Ok(m) => {
            info!(template = %tpl, hosts = resolver.len(), "multi-realm validator ready");
            (Some(Arc::new(m)), Some(Arc::new(resolver)))
        }
        Err(e) => {
            error!(error = %e, "multi-realm init failed");
            (None, None)
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = resolve_telemetry();
    init_tracing(&telemetry);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "expresso-notes starting"
    );

    let db = match resolve_database_config() {
        Some(cfg) => match create_db_pool(&cfg).await {
            Ok(pool) => Some(pool),
            Err(e) => {
                warn!(error = %e, "database unavailable; readiness degraded");
                None
            }
        },
        None => {
            warn!("database config missing; readiness degraded");
            None
        }
    };

    let search_url = env_string("SEARCH__URL").unwrap_or_default();
    let search_token = env_string("SEARCH__TOKEN").unwrap_or_default();
    if search_url.is_empty() {
        info!("SEARCH__URL unset — note full-text indexing disabled");
    }

    let oidc = resolve_oidc().await;
    if oidc.is_none() {
        warn!("AUTH__OIDC_ISSUER / AUTH__OIDC_AUDIENCE unset — auth in DEV header-mode (INSECURE)");
    }
    let (multi, resolver) = resolve_multi_realm();
    let http_addr = resolve_addr()?;

    let state = AppState::new(db, search_url, search_token);
    let app = api::router(state, oidc, multi, resolver, resolve_pat());
    let listener = tokio::net::TcpListener::bind(http_addr).await?;

    info!(addr = %http_addr, "HTTP API listening");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn default_port_is_8012() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("SERVER__PORT");
        std::env::remove_var("SERVER__HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), 8012);
    }

    #[test]
    fn resolve_db_none_when_url_missing() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("DATABASE__URL");
        assert!(resolve_database_config().is_none());
    }

    #[test]
    fn env_bool_parses_true() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("NOTES_TEST_BOOL", "true");
        assert!(env_bool("NOTES_TEST_BOOL", false));
        std::env::remove_var("NOTES_TEST_BOOL");
    }
}

//! expresso-contacts library — CardDAV, vCard parsing, addressbooks.

mod api;
mod carddav;
mod domain;
mod error;
mod events;
#[cfg(feature = "fuzzing")]
pub mod fuzz_entry;
mod metrics;
mod state;

use std::{env, net::SocketAddr, sync::Arc};

use tracing::{info, warn};

use expresso_core::config::{DatabaseConfig, TelemetryConfig};
use expresso_core::{create_db_pool, init_tracing};
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
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
fn env_u32(key: &str, default: u32) -> u32 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
fn env_i32(key: &str, default: i32) -> i32 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
fn env_bool(key: &str, default: bool) -> bool {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
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

fn resolve_multi_realm() -> (
    Option<Arc<expresso_auth_client::MultiRealmValidator>>,
    Option<Arc<expresso_auth_client::TenantResolver>>,
) {
    let tpl = match env_string("AUTH__OIDC_ISSUER_TEMPLATE") {
        Some(v) => v,
        None => return (None, None),
    };
    let audience = match env_string("AUTH__OIDC_AUDIENCE") {
        Some(v) => v,
        None => return (None, None),
    };
    let resolver = expresso_auth_client::TenantResolver::from_env("AUTH__TENANT_HOSTS");
    if resolver.is_empty() {
        warn!("AUTH__TENANT_HOSTS empty — multi-realm disabled");
        return (None, None);
    }
    match expresso_auth_client::MultiRealmValidator::new(tpl.clone(), audience.clone()) {
        Ok(m) => {
            info!(template = %tpl, hosts = resolver.len(), "multi-realm validator ready");
            (Some(Arc::new(m)), Some(Arc::new(resolver)))
        }
        Err(e) => {
            tracing::error!(error = %e, "multi-realm init failed");
            (None, None)
        }
    }
}

/// Library entry point: wire config, DB, and serve the contacts API.
pub async fn run() -> anyhow::Result<()> {
    let telemetry = resolve_telemetry();
    init_tracing(&telemetry);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "expresso-contacts starting"
    );

    // Pre-populate metric series so rate()/increase() work from first scrape.
    metrics::init();

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

    let http_addr = resolve_addr()?;
    let kc_basic = expresso_auth_client::KcBasicConfig::from_env_prefix("CARDDAV_KC")
        .map(expresso_auth_client::KcBasicAuthenticator::new);
    if kc_basic.is_some() {
        tracing::info!("CardDAV Keycloak Basic auth enabled");
    }
    if let Some(pool) = db.clone() {
        let retention = env_i32(
            "DAV_TOMBSTONE_RETENTION_DAYS",
            domain::tombstone_gc::DEFAULT_RETENTION_DAYS,
        );
        let every = env_u64(
            "DAV_TOMBSTONE_GC_INTERVAL_HOURS",
            domain::tombstone_gc::DEFAULT_INTERVAL_HOURS,
        );
        info!(
            retention_days = retention,
            interval_hours = every,
            "spawning tombstone GC"
        );
        domain::tombstone_gc::spawn(pool, retention, every);
    }
    // Sprint #23: opt-in NATS JetStream via NATS_URL.
    let bus = match std::env::var("NATS_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        Some(url) => match crate::events::ContactsEventBus::new_with_nats(&url).await {
            Ok(b) => {
                tracing::info!(nats_url=%url, "contacts EventBus with NATS enabled");
                b
            }
            Err(e) => {
                tracing::warn!(error=%e, "NATS init failed; noop bus");
                crate::events::ContactsEventBus::noop()
            }
        },
        None => crate::events::ContactsEventBus::noop(),
    };
    let search_url = env_string("SEARCH__URL").unwrap_or_default();
    let search_token = env_string("SEARCH__TOKEN").unwrap_or_default();
    if search_url.is_empty() {
        info!("SEARCH__URL unset — contact full-text indexing disabled");
    }
    let state = AppState::with_search(db, kc_basic, bus, search_url, search_token);
    // Per-tenant rate limiter (shared core; see expresso_core::ratelimit).
    let rate_cfg = expresso_core::ratelimit::RateLimitConfig::from_env();
    info!(
        rps = rate_cfg.rps,
        burst = rate_cfg.burst,
        "rate limiter armed"
    );
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
    if let Some(m) = multi {
        app = app.layer(axum::extract::Extension(m));
    }
    if let Some(r) = resolver {
        app = app.layer(axum::extract::Extension(r));
    }
    let listener = tokio::net::TcpListener::bind(http_addr).await?;

    info!(addr = %http_addr, "HTTP API listening");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn env_string_none_when_unset() {
        let key = "CONT_TEST_UNSET_19982";
        std::env::remove_var(key);
        assert!(env_string(key).is_none());
    }

    #[test]
    fn env_string_some_when_set() {
        let key = "CONT_TEST_STR_19982";
        std::env::set_var(key, "value");
        assert_eq!(env_string(key).as_deref(), Some("value"));
        std::env::remove_var(key);
    }

    #[test]
    fn env_string_none_for_whitespace() {
        let key = "CONT_TEST_WS_19982";
        std::env::set_var(key, "  ");
        assert!(env_string(key).is_none());
        std::env::remove_var(key);
    }

    #[test]
    fn env_u16_default_when_unset() {
        let key = "CONT_TEST_U16_UNSET_19982";
        std::env::remove_var(key);
        assert_eq!(env_u16(key, 7777), 7777);
    }

    #[test]
    fn env_u16_parsed_value() {
        let key = "CONT_TEST_U16_19982";
        std::env::set_var(key, "5050");
        assert_eq!(env_u16(key, 0), 5050);
        std::env::remove_var(key);
    }

    #[test]
    fn env_u32_default_when_unset() {
        let key = "CONT_TEST_U32_UNSET_19982";
        std::env::remove_var(key);
        assert_eq!(env_u32(key, 10), 10);
    }

    #[test]
    fn env_i32_default_when_unset() {
        let key = "CONT_TEST_I32_UNSET_19982";
        std::env::remove_var(key);
        assert_eq!(env_i32(key, -1), -1);
    }

    #[test]
    fn env_u64_default_when_unset() {
        let key = "CONT_TEST_U64_UNSET_19982";
        std::env::remove_var(key);
        assert_eq!(env_u64(key, 5), 5);
    }

    #[test]
    fn env_bool_default_when_unset() {
        let key = "CONT_TEST_BOOL_UNSET_19982";
        std::env::remove_var(key);
        assert!(!env_bool(key, false));
    }

    #[test]
    fn env_bool_true_when_set() {
        let key = "CONT_TEST_BOOL_T_19982";
        std::env::set_var(key, "true");
        assert!(env_bool(key, false));
        std::env::remove_var(key);
    }

    #[test]
    fn default_port_is_8003() {
        assert_eq!(DEFAULT_PORT, 8003);
    }

    #[test]
    fn default_host_is_all_interfaces() {
        assert_eq!(DEFAULT_HOST, "0.0.0.0");
    }

    #[test]
    fn resolve_addr_default_port_when_env_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("SERVER__HOST");
        std::env::remove_var("SERVER__PORT");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), DEFAULT_PORT);
    }

    #[test]
    fn resolve_database_config_none_when_url_missing() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("DATABASE__URL");
        assert!(resolve_database_config().is_none());
    }

    #[test]
    fn resolve_database_config_some_when_url_present() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("DATABASE__URL", "postgres://localhost/conttest");
        let cfg = resolve_database_config();
        assert!(cfg.is_some());
        assert_eq!(cfg.unwrap().url, "postgres://localhost/conttest");
        std::env::remove_var("DATABASE__URL");
    }

    #[test]
    fn resolve_telemetry_default_otlp_when_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("TELEMETRY__OTLP_ENDPOINT");
        let t = resolve_telemetry();
        assert_eq!(t.otlp_endpoint, DEFAULT_OTLP_ENDPOINT);
    }

    #[test]
    fn resolve_telemetry_log_json_false_by_default() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("TELEMETRY__LOG_JSON");
        let t = resolve_telemetry();
        assert!(!t.log_json);
    }
}

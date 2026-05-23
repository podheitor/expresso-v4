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

use std::{env, net::SocketAddr, sync::Arc};

use tracing::{error, info, warn};

use expresso_auth_client::{MultiRealmValidator, OidcConfig, OidcValidator, TenantResolver};

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


/// Build multi-realm auth (fase 2 do realm-per-tenant). Retorna (Some, Some)
/// quando AUTH__OIDC_ISSUER_TEMPLATE (com placeholder `{realm}`) +
/// AUTH__TENANT_HOSTS estão setados. Caso contrário retorna (None, None) e
/// o serviço usa apenas single-realm (Authenticated).
fn resolve_multi_realm() -> (Option<Arc<MultiRealmValidator>>, Option<Arc<TenantResolver>>) {
    let tpl = match env_string("AUTH__OIDC_ISSUER_TEMPLATE") { Some(v) => v, None => return (None, None) };
    let audience = match env_string("AUTH__OIDC_AUDIENCE")   { Some(v) => v, None => return (None, None) };
    let resolver = TenantResolver::from_env("AUTH__TENANT_HOSTS");
    if resolver.is_empty() {
        warn!("AUTH__TENANT_HOSTS empty — multi-realm disabled");
        return (None, None);
    }
    match MultiRealmValidator::new(tpl.clone(), audience.clone()) {
        Ok(m)  => {
            info!(template = %tpl, hosts = resolver.len(), "multi-realm validator ready");
            (Some(Arc::new(m)), Some(Arc::new(resolver)))
        }
        Err(e) => { error!(error = %e, "multi-realm init failed"); (None, None) }
    }
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

    let oidc = resolve_oidc().await;
    if oidc.is_none() {
        warn!("AUTH__OIDC_ISSUER / AUTH__OIDC_AUDIENCE unset — auth in DEV header-mode (INSECURE)");
    }
    let (multi, resolver) = resolve_multi_realm();
    let http_addr = resolve_addr()?;
    let state = AppState::new(db, matrix);
    let app = api::router(state, oidc, multi, resolver);
    let listener = tokio::net::TcpListener::bind(http_addr).await?;

    info!(addr = %http_addr, "HTTP API listening");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_string_returns_none_when_unset() {
        let key = "CHAT_TEST_UNSET_ZZZ_19972";
        std::env::remove_var(key);
        assert!(env_string(key).is_none());
    }

    #[test]
    fn env_string_returns_value_when_set() {
        let key = "CHAT_TEST_STR_19972";
        std::env::set_var(key, "hello");
        assert_eq!(env_string(key).as_deref(), Some("hello"));
        std::env::remove_var(key);
    }

    #[test]
    fn env_string_returns_none_for_whitespace_only() {
        let key = "CHAT_TEST_WS_19972";
        std::env::set_var(key, "   ");
        assert!(env_string(key).is_none());
        std::env::remove_var(key);
    }

    #[test]
    fn env_u16_returns_default_when_unset() {
        let key = "CHAT_TEST_U16_UNSET_19972";
        std::env::remove_var(key);
        assert_eq!(env_u16(key, 9999), 9999);
    }

    #[test]
    fn env_u16_returns_parsed_value() {
        let key = "CHAT_TEST_U16_19972";
        std::env::set_var(key, "8080");
        assert_eq!(env_u16(key, 0), 8080);
        std::env::remove_var(key);
    }

    #[test]
    fn env_u16_returns_default_on_invalid() {
        let key = "CHAT_TEST_U16_INV_19972";
        std::env::set_var(key, "not_a_number");
        assert_eq!(env_u16(key, 42), 42);
        std::env::remove_var(key);
    }

    #[test]
    fn env_u32_returns_default_when_unset() {
        let key = "CHAT_TEST_U32_UNSET_19972";
        std::env::remove_var(key);
        assert_eq!(env_u32(key, 20), 20);
    }

    #[test]
    fn env_u32_returns_parsed_value() {
        let key = "CHAT_TEST_U32_19972";
        std::env::set_var(key, "100");
        assert_eq!(env_u32(key, 0), 100);
        std::env::remove_var(key);
    }

    #[test]
    fn env_u64_returns_default_when_unset() {
        let key = "CHAT_TEST_U64_UNSET_19972";
        std::env::remove_var(key);
        assert_eq!(env_u64(key, 5), 5);
    }

    #[test]
    fn env_u64_returns_parsed_value() {
        let key = "CHAT_TEST_U64_19972";
        std::env::set_var(key, "30");
        assert_eq!(env_u64(key, 0), 30);
        std::env::remove_var(key);
    }

    #[test]
    fn env_bool_returns_default_when_unset() {
        let key = "CHAT_TEST_BOOL_UNSET_19972";
        std::env::remove_var(key);
        assert!(!env_bool(key, false));
        assert!(env_bool(key, true));
    }

    #[test]
    fn env_bool_returns_true_when_set_true() {
        let key = "CHAT_TEST_BOOL_T_19972";
        std::env::set_var(key, "true");
        assert!(env_bool(key, false));
        std::env::remove_var(key);
    }

    #[test]
    fn env_bool_returns_false_when_set_false() {
        let key = "CHAT_TEST_BOOL_F_19972";
        std::env::set_var(key, "false");
        assert!(!env_bool(key, true));
        std::env::remove_var(key);
    }

    #[test]
    fn default_port_is_8004() {
        assert_eq!(DEFAULT_PORT, 8004);
    }

    #[test]
    fn default_host_is_all_interfaces() {
        assert_eq!(DEFAULT_HOST, "0.0.0.0");
    }

    #[test]
    fn resolve_addr_uses_defaults_when_env_unset() {
        std::env::remove_var("SERVER__HOST");
        std::env::remove_var("SERVER__PORT");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), DEFAULT_PORT);
    }

    #[test]
    fn resolve_database_config_none_when_url_missing() {
        std::env::remove_var("DATABASE__URL");
        assert!(resolve_database_config().is_none());
    }

    #[test]
    fn resolve_database_config_some_when_url_set() {
        let key = "DATABASE__URL";
        std::env::set_var(key, "postgres://localhost/test");
        let cfg = resolve_database_config();
        assert!(cfg.is_some());
        assert_eq!(cfg.unwrap().url, "postgres://localhost/test");
        std::env::remove_var(key);
    }

    #[test]
    fn resolve_matrix_config_none_when_hs_url_missing() {
        std::env::remove_var("MATRIX__HS_URL");
        std::env::remove_var("MATRIX__SERVER_NAME");
        assert!(resolve_matrix_config().is_none());
    }

    #[test]
    fn resolve_telemetry_uses_default_otlp_when_unset() {
        std::env::remove_var("TELEMETRY__OTLP_ENDPOINT");
        let t = resolve_telemetry();
        assert_eq!(t.otlp_endpoint, DEFAULT_OTLP_ENDPOINT);
    }

    #[test]
    fn resolve_telemetry_log_json_false_by_default() {
        std::env::remove_var("TELEMETRY__LOG_JSON");
        let t = resolve_telemetry();
        assert!(!t.log_json);
    }
}

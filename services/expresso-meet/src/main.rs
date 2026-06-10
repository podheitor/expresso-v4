//! expresso-meet service entrypoint — Jitsi Meet BFF.
//!
//! Role: mint JWT tokens for Jitsi (HS256 with shared app_secret), register
//! meetings + participant ACL in Postgres, expose REST for the Expresso UI.
//! Rooms themselves are ephemeral on Jitsi — we only control access.

mod api;
mod domain;
mod error;
mod invite_mail;
mod jitsi;
mod state;
pub mod webhook;

use std::{env, net::SocketAddr, sync::Arc};

use tracing::{error, info, warn};

use expresso_auth_client::{MultiRealmValidator, OidcConfig, OidcValidator, TenantResolver};

use expresso_core::{
    config::{DatabaseConfig, TelemetryConfig},
    create_db_pool, init_tracing, DbPool,
};
use state::AppState;

/// How often the auto-end reaper sweeps for ended meetings.
const REAPER_INTERVAL_SECS: u64 = 300;

/// Background loop: every few minutes, archive meetings whose `ends_at` has
/// passed and that aren't already archived. Recurring meetings are left alone
/// (their `ends_at` marks one instance, not the series).
async fn meeting_reaper(pool: DbPool) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(REAPER_INTERVAL_SECS));
    loop {
        tick.tick().await;
        let res = sqlx::query(
            "UPDATE meetings SET is_archived = TRUE \
             WHERE is_archived = FALSE AND is_recurring = FALSE \
               AND ends_at IS NOT NULL AND ends_at < now()",
        )
        .execute(&pool)
        .await;
        match res {
            Ok(r) if r.rows_affected() > 0 => {
                info!(
                    count = r.rows_affected(),
                    "auto-ended meetings past ends_at"
                )
            }
            Ok(_) => {}
            Err(e) => warn!(error = %e, "meeting reaper sweep failed"),
        }
    }
}

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
fn env_u16(key: &str, d: u16) -> u16 {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn env_u32(key: &str, d: u32) -> u32 {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn env_u64(key: &str, d: u64) -> u64 {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn env_i64(key: &str, d: i64) -> i64 {
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

fn resolve_jitsi_config() -> Option<jitsi::JitsiConfig> {
    let app_id = env_string("JITSI__APP_ID")?;
    let app_secret = env_string("JITSI__APP_SECRET")?;
    let domain = env_string("JITSI__DOMAIN")?;
    Some(jitsi::JitsiConfig {
        app_id,
        app_secret,
        domain,
        jwt_ttl: env_i64("JITSI__JWT_TTL_SECS", DEFAULT_JWT_TTL_SECS),
        room_prefix: env_string("JITSI__ROOM_PREFIX")
            .unwrap_or_else(|| DEFAULT_ROOM_PREFIX.to_string()),
    })
}

/// Build an `OidcValidator` from env. Returns `None` when the issuer/audience
/// pair is unset (service runs in dev-header-auth mode with a loud warning).
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
        "expresso-meet starting"
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

    let jitsi = resolve_jitsi_config().map(jitsi::Jitsi::new);
    if jitsi.is_none() {
        warn!("jitsi config missing (JITSI__APP_ID + JITSI__APP_SECRET + JITSI__DOMAIN); meeting routes degraded");
    }

    let oidc = resolve_oidc().await;
    if oidc.is_none() {
        warn!("AUTH__OIDC_ISSUER / AUTH__OIDC_AUDIENCE unset — auth in DEV header-mode (INSECURE)");
    }
    let (multi, resolver) = resolve_multi_realm();
    let http_addr = resolve_addr()?;
    let webhook = webhook::WebhookConfig::from_env();
    if webhook.is_some() {
        info!("webhook dispatch enabled (MEET__WEBHOOK_URL)");
    }
    let invite_mailer = invite_mail::InviteMailer::from_env();
    if invite_mailer.is_some() {
        info!("invite mailer enabled (MEET__SMTP_HOST)");
    }
    let state = AppState::new(db, jitsi, webhook, invite_mailer);
    // Auto-end reaper: periodically archive meetings whose scheduled end has
    // passed, so stale rooms don't linger in the active list.
    if let Some(pool) = state.db().cloned() {
        tokio::spawn(async move { meeting_reaper(pool).await });
    }
    let app = api::router(state, oidc, multi, resolver, resolve_pat());
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
        let key = "MEET_TEST_UNSET_ZZZ_19975";
        std::env::remove_var(key);
        assert!(env_string(key).is_none());
    }

    #[test]
    fn env_string_some_when_set() {
        let key = "MEET_TEST_STR_19975";
        std::env::set_var(key, "value");
        assert_eq!(env_string(key).as_deref(), Some("value"));
        std::env::remove_var(key);
    }

    #[test]
    fn env_string_none_for_whitespace() {
        let key = "MEET_TEST_WS_19975";
        std::env::set_var(key, "  ");
        assert!(env_string(key).is_none());
        std::env::remove_var(key);
    }

    #[test]
    fn env_u16_default_when_unset() {
        let key = "MEET_TEST_U16_UNSET_19975";
        std::env::remove_var(key);
        assert_eq!(env_u16(key, 1234), 1234);
    }

    #[test]
    fn env_u16_parsed_value() {
        let key = "MEET_TEST_U16_19975";
        std::env::set_var(key, "9090");
        assert_eq!(env_u16(key, 0), 9090);
        std::env::remove_var(key);
    }

    #[test]
    fn env_u32_default_when_unset() {
        let key = "MEET_TEST_U32_UNSET_19975";
        std::env::remove_var(key);
        assert_eq!(env_u32(key, 10), 10);
    }

    #[test]
    fn env_u32_parsed_value() {
        let key = "MEET_TEST_U32_19975";
        std::env::set_var(key, "50");
        assert_eq!(env_u32(key, 0), 50);
        std::env::remove_var(key);
    }

    #[test]
    fn env_u64_default_when_unset() {
        let key = "MEET_TEST_U64_UNSET_19975";
        std::env::remove_var(key);
        assert_eq!(env_u64(key, 5), 5);
    }

    #[test]
    fn env_i64_default_when_unset() {
        let key = "MEET_TEST_I64_UNSET_19975";
        std::env::remove_var(key);
        assert_eq!(env_i64(key, 3600), 3600);
    }

    #[test]
    fn env_i64_parsed_value() {
        let key = "MEET_TEST_I64_19975";
        std::env::set_var(key, "7200");
        assert_eq!(env_i64(key, 0), 7200);
        std::env::remove_var(key);
    }

    #[test]
    fn env_bool_default_false_when_unset() {
        let key = "MEET_TEST_BOOL_UNSET_19975";
        std::env::remove_var(key);
        assert!(!env_bool(key, false));
    }

    #[test]
    fn env_bool_true_when_set() {
        let key = "MEET_TEST_BOOL_T_19975";
        std::env::set_var(key, "true");
        assert!(env_bool(key, false));
        std::env::remove_var(key);
    }

    #[test]
    fn default_port_is_8011() {
        assert_eq!(DEFAULT_PORT, 8011);
    }

    #[test]
    fn default_host_is_all_interfaces() {
        assert_eq!(DEFAULT_HOST, "0.0.0.0");
    }

    #[test]
    fn default_jwt_ttl_is_3600() {
        assert_eq!(DEFAULT_JWT_TTL_SECS, 3600);
    }

    #[test]
    fn default_room_prefix_is_exp() {
        assert_eq!(DEFAULT_ROOM_PREFIX, "exp-");
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
        std::env::set_var("DATABASE__URL", "postgres://localhost/meettest");
        let cfg = resolve_database_config();
        assert!(cfg.is_some());
        assert_eq!(cfg.unwrap().url, "postgres://localhost/meettest");
        std::env::remove_var("DATABASE__URL");
    }

    #[test]
    fn resolve_jitsi_config_none_when_app_id_missing() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("JITSI__APP_ID");
        std::env::remove_var("JITSI__APP_SECRET");
        std::env::remove_var("JITSI__DOMAIN");
        assert!(resolve_jitsi_config().is_none());
    }

    #[test]
    fn resolve_jitsi_config_none_when_partial_config() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("JITSI__APP_ID", "myapp");
        std::env::remove_var("JITSI__APP_SECRET");
        std::env::remove_var("JITSI__DOMAIN");
        assert!(resolve_jitsi_config().is_none());
        std::env::remove_var("JITSI__APP_ID");
    }

    #[test]
    fn resolve_jitsi_config_some_when_full_config() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("JITSI__APP_ID", "expresso");
        std::env::set_var("JITSI__APP_SECRET", "secret123");
        std::env::set_var("JITSI__DOMAIN", "meet.example.com");
        std::env::remove_var("JITSI__JWT_TTL_SECS");
        std::env::remove_var("JITSI__ROOM_PREFIX");
        let cfg = resolve_jitsi_config();
        assert!(cfg.is_some());
        let cfg = cfg.unwrap();
        assert_eq!(cfg.app_id, "expresso");
        assert_eq!(cfg.domain, "meet.example.com");
        assert_eq!(cfg.jwt_ttl, DEFAULT_JWT_TTL_SECS);
        assert_eq!(cfg.room_prefix, DEFAULT_ROOM_PREFIX);
        std::env::remove_var("JITSI__APP_ID");
        std::env::remove_var("JITSI__APP_SECRET");
        std::env::remove_var("JITSI__DOMAIN");
    }

    #[test]
    fn resolve_addr_uses_default_port_when_env_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("SERVER__HOST");
        std::env::remove_var("SERVER__PORT");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), DEFAULT_PORT);
    }

    #[test]
    fn resolve_telemetry_uses_default_otlp_when_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("TELEMETRY__OTLP_ENDPOINT");
        let t = resolve_telemetry();
        assert_eq!(t.otlp_endpoint, DEFAULT_OTLP_ENDPOINT);
    }
}

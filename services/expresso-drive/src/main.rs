//! expresso-drive service entrypoint

mod api;
mod domain;
mod error;
mod state;

use std::{env, net::SocketAddr, path::PathBuf, sync::Arc};

use tracing::{info, warn};

use expresso_core::{create_db_pool, init_tracing, report_rls_posture};
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

    // Surface real tenant-isolation posture at boot. Drive uses FORCE RLS on
    // all `drive_*` tables, but isolation only fires when (a) the DB role
    // does NOT have BYPASSRLS and (b) handlers run inside a tx with
    // `app.tenant_id` set. Today's handlers rely on explicit WHERE clauses;
    // this log makes the gap explicit instead of hidden.
    if let Some(p) = db.as_ref() {
        let _ = report_rls_posture(p, &[
            "drive_files",
            "drive_file_versions",
            "drive_shares",
            "drive_quotas",
            "drive_uploads",
        ]).await;
    }

    let data_root = PathBuf::from(
        env_string("DRIVE__DATA_ROOT").unwrap_or_else(|| DEFAULT_DATA_ROOT.into())
    );
    if let Err(e) = tokio::fs::create_dir_all(&data_root).await {
        warn!(error=%e, path=%data_root.display(), "cannot create data_root");
    }

    let addr  = resolve_addr()?;
    let state = AppState::new(db.clone(), data_root.clone());

    api::init_wopi_metrics();

    // tus.io expiration — hourly GC of abandoned uploads + matching .part blobs.
    if let Some(pool) = db.clone() {
        let root = data_root.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(3600));
            ticker.tick().await; // fires immediately
            loop {
                let repo = domain::upload::UploadRepo::new(&pool);
                let keys = repo.list_expired_keys().await.unwrap_or_default();
                match repo.purge_expired().await {
                    Ok(n) if n > 0 || !keys.is_empty() => {
                        for k in &keys {
                            let p = root.join(format!("{k}.part"));
                            let _ = tokio::fs::remove_file(&p).await;
                        }
                        info!(rows = n, blobs = keys.len(), "drive_uploads GC");
                    }
                    Ok(_) => {}
                    Err(e) => warn!(error = %e, "drive_uploads purge failed"),
                }
                ticker.tick().await;
            }
        });
    }

    let (multi, resolver) = resolve_multi_realm();
    let mut app = api::router(state);
    if let Some(m) = multi    { app = app.layer(axum::extract::Extension(m)); }
    if let Some(r) = resolver { app = app.layer(axum::extract::Extension(r)); }
    let lst   = tokio::net::TcpListener::bind(addr).await?;

    info!(%addr, "HTTP API listening");
    axum::serve(lst, app).await?;
    Ok(())
}

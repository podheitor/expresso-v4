//! expresso-mail library — SMTP/LMTP/Submission intake, IMAP4rev1 server, webmail REST API.
//!
//! Listeners (ports are config-driven; defaults shown):
//!   - :8001  HTTP REST (webmail API + /mail/send + iTIP dispatch)
//!   - :25    SMTP — inbound MTA reception
//!   - :587   SMTP Submission — STARTTLS + AUTH PLAIN/LOGIN (only when TLS wired)
//!   - :24    LMTP — Postfix → app delivery
//!   - :143   IMAP4rev1 — LOGIN, LIST, SELECT, FETCH, STORE, EXPUNGE, CLOSE, NOOP
//!   - :993   IMAPS — IMAP4rev1 over implicit TLS (RFC 8314, only when TLS wired)
//!   - :465   SMTPS — SMTP over implicit TLS (RFC 8314, only when TLS wired)

mod api;
mod bootstrap;
mod dkim;
mod error;
#[cfg(feature = "fuzzing")]
pub mod fuzz_entry;
mod imap;
mod imip;
mod ingest;
mod lmtp;
mod scanner;
mod sieve;
mod smtp;
mod state;

use std::{net::SocketAddr, sync::Arc};
use tokio::{signal, task::JoinSet};
use tracing::info;

use expresso_core::{create_db_pool, create_redis_pool, init_tracing, run_migrations, AppConfig};
use state::AppState;

fn env_string(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
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
        tracing::warn!("AUTH__TENANT_HOSTS empty — multi-realm disabled");
        return (None, None);
    }
    match expresso_auth_client::MultiRealmValidator::new(tpl.clone(), audience.clone()) {
        Ok(m) => {
            tracing::info!(template = %tpl, hosts = resolver.len(), "multi-realm validator ready");
            (Some(Arc::new(m)), Some(Arc::new(resolver)))
        }
        Err(e) => {
            tracing::error!(error = %e, "multi-realm init failed");
            (None, None)
        }
    }
}

/// Build the PAT resolver when `AUTH__URL` (expresso-auth base URL) is set.
/// Absent → None and the PAT auth path stays off (JWT-only), so this is opt-in.
fn resolve_pat() -> Option<Arc<expresso_auth_client::PatResolver>> {
    let auth_url = env_string("AUTH__URL")?;
    tracing::info!(%auth_url, "PAT resolver enabled");
    Some(Arc::new(expresso_auth_client::PatResolver::new(
        auth_url,
        reqwest::Client::new(),
    )))
}

/// Library entry point: wire config, DB, and spawn all listeners.
pub async fn run() -> anyhow::Result<()> {
    // ── Config ─────────────────────────────────────────────────────────────────
    let cfg = AppConfig::from_env()?;
    init_tracing(&cfg.telemetry);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "expresso-mail starting"
    );

    // ── Database ───────────────────────────────────────────────────────────────
    let db = create_db_pool(&cfg.database).await?;
    run_migrations(&db).await?;
    let redis = create_redis_pool(&cfg.redis)?;

    // Build S3 object store if configured
    let store = if !cfg.s3.endpoint.is_empty() {
        Some(
            expresso_storage::ObjectStore::new(
                &cfg.s3.endpoint,
                &cfg.s3.bucket,
                &cfg.s3.access_key,
                &cfg.s3.secret_key,
                &cfg.s3.region,
            )
            .await,
        )
    } else {
        None
    };
    let state = match store {
        Some(s) => {
            info!("S3 object store configured (bucket={})", cfg.s3.bucket);
            AppState::with_store(cfg.clone(), db, redis, s)
        }
        None => {
            info!("S3 not configured — using local filesystem for raw messages");
            AppState::new(cfg.clone(), db, redis)
        }
    };
    // Load DKIM signer if configured
    let state = if let (Some(sel), Some(key_path)) = (
        &cfg.mail_server.dkim_selector,
        &cfg.mail_server.dkim_key_path,
    ) {
        match dkim::DkimSignerState::from_pem_file(&cfg.mail_server.domain, sel, key_path) {
            Ok(signer) => state.set_dkim(signer),
            Err(e) => {
                tracing::warn!(error = %e, "DKIM signer not loaded — outbound mail will be unsigned");
                state
            }
        }
    } else {
        info!("DKIM not configured — outbound mail will be unsigned");
        state
    };
    if dev_bootstrap_enabled() {
        bootstrap::ensure_dev_bootstrap(&state).await?;
    } else {
        info!("dev bootstrap disabled (set EXPRESSO_DEV_BOOTSTRAP=true to enable)");
    }

    // ── Launch servers in parallel ─────────────────────────────────────────────
    let mut set = JoinSet::new();

    // HTTP API
    let http_addr: SocketAddr = format!("{}:{}", cfg.server.host, cfg.server.port).parse()?;
    let http_state = state.clone();
    let (multi, resolver) = resolve_multi_realm();
    let pat = resolve_pat();
    set.spawn(async move {
        let mut router = api::router(http_state);
        if let Some(m) = multi {
            router = router.layer(axum::extract::Extension(m));
        }
        if let Some(r) = resolver {
            router = router.layer(axum::extract::Extension(r));
        }
        if let Some(p) = pat {
            router = router.layer(axum::extract::Extension(p));
        }
        let listener = tokio::net::TcpListener::bind(http_addr).await?;
        info!(addr = %http_addr, "HTTP API listening");
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await?;
        anyhow::Ok(())
    });

    // SMTP
    let smtp_addr: SocketAddr = format!("0.0.0.0:{}", cfg.mail_server.smtp_port).parse()?;
    let smtp_state = state.clone();
    set.spawn(async move { smtp::serve(smtp_state, smtp_addr).await });

    // SMTP Submission (587, STARTTLS + AUTH required) — only if TLS configured
    if cfg.mail_server.tls_cert.is_some() && cfg.mail_server.tls_key.is_some() {
        let sub_addr: SocketAddr =
            format!("0.0.0.0:{}", cfg.mail_server.submission_port).parse()?;
        let sub_state = state.clone();
        set.spawn(async move { smtp::submission::serve(sub_state, sub_addr).await });
    } else {
        info!("submission (587) disabled — mail_server.tls_cert/tls_key not set");
    }

    // SMTPS — implicit TLS (port 465, RFC 8314) — only when TLS configured
    if cfg.mail_server.tls_cert.is_some() && cfg.mail_server.tls_key.is_some() {
        let smtps_addr: SocketAddr = format!("0.0.0.0:{}", cfg.mail_server.smtps_port).parse()?;
        let smtps_state = state.clone();
        set.spawn(async move { smtp::serve_smtps(smtps_state, smtps_addr).await });
    } else {
        info!("SMTPS (465) disabled — mail_server.tls_cert/tls_key not set");
    }

    // LMTP (Postfix → app delivery)
    let lmtp_addr: SocketAddr = format!("0.0.0.0:{}", cfg.mail_server.lmtp_port).parse()?;
    let lmtp_state = state.clone();
    set.spawn(async move { lmtp::serve(lmtp_state, lmtp_addr).await });

    // Scheduled send worker — polls every 30 s for messages with deliver_at <= now()
    let sched_state = state.clone();
    set.spawn(async move { scheduled_send_worker(sched_state).await });

    // Snooze waker — wakes snoozed messages when snooze_until <= now()
    {
        let snooze_interval = std::env::var("MAIL_SNOOZE_WAKER_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60u64);
        let snooze_pool = state.db().clone();
        tracing::info!(interval_secs = snooze_interval, "spawning snooze waker");
        api::snooze::spawn_waker(snooze_pool, snooze_interval);
    }

    // Vacation auto-deactivation worker — hourly, deactivates when deactivate_at <= now()
    api::vacation::spawn_deactivation_worker(state.db().clone());

    // IMAP4rev1 (plain, port 143)
    let imap_addr: SocketAddr = format!("0.0.0.0:{}", cfg.mail_server.imap_port).parse()?;
    let imap_state = state.clone();
    set.spawn(async move { imap::serve(imap_state, imap_addr).await });

    // IMAPS — implicit TLS (port 993, RFC 8314) — only when TLS configured
    if cfg.mail_server.tls_cert.is_some() && cfg.mail_server.tls_key.is_some() {
        let imaps_addr: SocketAddr = format!("0.0.0.0:{}", cfg.mail_server.imaps_port).parse()?;
        let imaps_state = state.clone();
        set.spawn(async move { imap::serve_tls(imaps_state, imaps_addr).await });
    } else {
        info!("IMAPS (993) disabled — mail_server.tls_cert/tls_key not set");
    }

    // ── Wait for any task to finish (usually shutdown signal) ──────────────────
    while let Some(result) = set.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::error!(error = %e, "server task error"),
            Err(e) => tracing::error!(error = %e, "task join error"),
        }
    }

    info!("expresso-mail shutdown complete");
    Ok(())
}

/// Background worker: every 5 s, find messages with deliver_at <= now() and relay them.
/// On SMTP failure the row is left untouched and retried on the next tick.
async fn scheduled_send_worker(state: AppState) -> anyhow::Result<()> {
    // 5 s tick keeps undo-send (POST /mail/send-with-undo, 5–30 s window)
    // delivering promptly once the window elapses. Cheap: the due query hits
    // messages_deliver_at_idx and is LIMIT 50.
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        interval.tick().await;
        if let Err(e) = dispatch_due_messages(&state).await {
            tracing::error!(error = %e, "scheduled_send_worker tick failed");
        }
    }
}

async fn dispatch_due_messages(state: &AppState) -> anyhow::Result<()> {
    use lettre::{address::Envelope, Address, AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
    use tokio::fs;

    #[derive(sqlx::FromRow)]
    struct DueRow {
        id: uuid::Uuid,
        tenant_id: uuid::Uuid,
        body_path: String,
        from_addr: Option<String>,
        to_addrs: sqlx::types::Json<Vec<String>>,
    }

    let due: Vec<DueRow> = sqlx::query_as(
        "SELECT m.id, m.tenant_id, m.body_path, m.from_addr, m.to_addrs \
         FROM messages m \
         WHERE m.deliver_at IS NOT NULL AND m.deliver_at <= now() \
         ORDER BY m.deliver_at \
         LIMIT 50",
    )
    .fetch_all(state.db())
    .await?;

    let data_root = std::path::PathBuf::from(
        std::env::var("MAIL__DATA_ROOT").unwrap_or_else(|_| "/var/lib/expresso/mail".into()),
    );
    let smtp_host = state.cfg().mail_server.relay_host.clone();
    let smtp_port = state.cfg().mail_server.relay_port;
    let mailer = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&smtp_host)
        .port(smtp_port)
        .build();

    for row in due {
        let raw = match fs::read(data_root.join(&row.body_path)).await {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(msg_id = %row.id, error = %e, "scheduled send: cannot read body");
                continue;
            }
        };

        let from: Option<Address> = row.from_addr.as_deref().and_then(|s| s.parse().ok());
        let to: Vec<Address> = row
            .to_addrs
            .0
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();
        if to.is_empty() {
            tracing::error!(msg_id = %row.id, "scheduled send: no valid to addresses");
            continue;
        }
        let envelope = match Envelope::new(from, to) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(msg_id = %row.id, error = %e, "scheduled send: bad envelope");
                continue;
            }
        };

        match mailer.send_raw(&envelope, &raw).await {
            Ok(_) => {
                if let Err(e) = sqlx::query(
                    "UPDATE messages \
                     SET deliver_at = NULL, \
                         flags = array(SELECT DISTINCT unnest(flags || ARRAY[$2::text])) \
                     WHERE id = $1",
                )
                .bind(row.id)
                .bind(r"\Seen")
                .execute(state.db())
                .await
                {
                    tracing::error!(msg_id = %row.id, error = %e, "scheduled send: status update failed");
                } else {
                    tracing::info!(target: "audit",
                        event = "mail.scheduled_sent",
                        msg_id = %row.id, tenant_id = %row.tenant_id);
                }
            }
            Err(e) => {
                tracing::warn!(msg_id = %row.id, error = %e, "scheduled send: relay failed, will retry");
            }
        }
    }
    Ok(())
}

fn dev_bootstrap_enabled() -> bool {
    matches!(
        std::env::var("EXPRESSO_DEV_BOOTSTRAP").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
    )
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("shutdown signal received");
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn env_string_none_when_unset() {
        let key = "MAIL_TEST_UNSET_19981";
        std::env::remove_var(key);
        assert!(env_string(key).is_none());
    }

    #[test]
    fn env_string_some_when_set() {
        let key = "MAIL_TEST_STR_19981";
        std::env::set_var(key, "hello");
        assert_eq!(env_string(key).as_deref(), Some("hello"));
        std::env::remove_var(key);
    }

    #[test]
    fn env_string_none_for_whitespace() {
        let key = "MAIL_TEST_WS_19981";
        std::env::set_var(key, "   ");
        assert!(env_string(key).is_none());
        std::env::remove_var(key);
    }

    #[test]
    fn dev_bootstrap_enabled_false_when_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("EXPRESSO_DEV_BOOTSTRAP");
        assert!(!dev_bootstrap_enabled());
    }

    #[test]
    fn dev_bootstrap_enabled_true_for_1() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("EXPRESSO_DEV_BOOTSTRAP", "1");
        assert!(dev_bootstrap_enabled());
        std::env::remove_var("EXPRESSO_DEV_BOOTSTRAP");
    }

    #[test]
    fn dev_bootstrap_enabled_true_for_true_lowercase() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("EXPRESSO_DEV_BOOTSTRAP", "true");
        assert!(dev_bootstrap_enabled());
        std::env::remove_var("EXPRESSO_DEV_BOOTSTRAP");
    }

    #[test]
    fn dev_bootstrap_enabled_true_for_true_uppercase() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("EXPRESSO_DEV_BOOTSTRAP", "TRUE");
        assert!(dev_bootstrap_enabled());
        std::env::remove_var("EXPRESSO_DEV_BOOTSTRAP");
    }

    #[test]
    fn dev_bootstrap_enabled_true_for_yes() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("EXPRESSO_DEV_BOOTSTRAP", "yes");
        assert!(dev_bootstrap_enabled());
        std::env::remove_var("EXPRESSO_DEV_BOOTSTRAP");
    }

    #[test]
    fn dev_bootstrap_enabled_true_for_yes_uppercase() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("EXPRESSO_DEV_BOOTSTRAP", "YES");
        assert!(dev_bootstrap_enabled());
        std::env::remove_var("EXPRESSO_DEV_BOOTSTRAP");
    }

    #[test]
    fn dev_bootstrap_enabled_false_for_zero() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("EXPRESSO_DEV_BOOTSTRAP", "0");
        assert!(!dev_bootstrap_enabled());
        std::env::remove_var("EXPRESSO_DEV_BOOTSTRAP");
    }

    #[test]
    fn dev_bootstrap_enabled_false_for_false() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("EXPRESSO_DEV_BOOTSTRAP", "false");
        assert!(!dev_bootstrap_enabled());
        std::env::remove_var("EXPRESSO_DEV_BOOTSTRAP");
    }

    #[test]
    fn dev_bootstrap_enabled_false_for_no() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("EXPRESSO_DEV_BOOTSTRAP", "no");
        assert!(!dev_bootstrap_enabled());
        std::env::remove_var("EXPRESSO_DEV_BOOTSTRAP");
    }

    #[test]
    fn dev_bootstrap_enabled_false_for_empty_string() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("EXPRESSO_DEV_BOOTSTRAP", "");
        assert!(!dev_bootstrap_enabled());
        std::env::remove_var("EXPRESSO_DEV_BOOTSTRAP");
    }

    #[test]
    fn dev_bootstrap_enabled_false_for_random_string() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("EXPRESSO_DEV_BOOTSTRAP", "enabled");
        assert!(!dev_bootstrap_enabled());
        std::env::remove_var("EXPRESSO_DEV_BOOTSTRAP");
    }

    #[test]
    fn env_string_trims_and_rejects_blank() {
        let key = "MAIL_TEST_BLANK_19981";
        std::env::set_var(key, "\t");
        assert!(env_string(key).is_none());
        std::env::remove_var(key);
    }

    #[test]
    fn env_string_preserves_value_with_spaces_inside() {
        let key = "MAIL_TEST_SPACES_INSIDE_19981";
        std::env::set_var(key, "hello world");
        assert_eq!(env_string(key).as_deref(), Some("hello world"));
        std::env::remove_var(key);
    }

    #[test]
    fn dev_bootstrap_enabled_returns_bool() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("EXPRESSO_DEV_BOOTSTRAP", "1");
        assert!(dev_bootstrap_enabled());
        std::env::remove_var("EXPRESSO_DEV_BOOTSTRAP");
        assert!(!dev_bootstrap_enabled());
    }
}

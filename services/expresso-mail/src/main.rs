//! expresso-mail — SMTP intake + IMAP stub + REST API for webmail
//!
//! Listeners:
//!   - :8001  HTTP REST (webmail API)
//!   - :25    SMTP (inbound MTA reception)
//!   - :143   IMAP (stub — Phase 2)

mod api;
mod bootstrap;
mod error;
mod imap;
mod lmtp;
mod ingest;
mod imip;
mod dkim;
mod sieve;
mod smtp;
mod state;

use std::net::SocketAddr;
use tokio::{signal, task::JoinSet};
use tracing::info;

use expresso_core::{create_db_pool, create_redis_pool, init_tracing, run_migrations, AppConfig};
use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
    let state = if let (Some(sel), Some(key_path)) = (&cfg.mail_server.dkim_selector, &cfg.mail_server.dkim_key_path) {
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
    set.spawn(async move {
        let router = api::router(http_state);
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

    // LMTP (Postfix → app delivery)
    let lmtp_addr: SocketAddr = format!("0.0.0.0:{}", cfg.mail_server.lmtp_port).parse()?;
    let lmtp_state = state.clone();
    set.spawn(async move { lmtp::serve(lmtp_state, lmtp_addr).await });

    // IMAP (stub)
    let imap_addr: SocketAddr = format!("0.0.0.0:{}", cfg.mail_server.imap_port).parse()?;
    let imap_state = state.clone();
    set.spawn(async move { imap::serve(imap_state, imap_addr).await });

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

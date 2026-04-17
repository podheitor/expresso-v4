//! expresso-mail — SMTP intake + IMAP stub + REST API for webmail
//!
//! Listeners:
//!   - :8001  HTTP REST (webmail API)
//!   - :25    SMTP (inbound MTA reception)
//!   - :143   IMAP (stub — Phase 2)

mod api;
mod error;
mod imap;
mod smtp;
mod state;

use std::net::SocketAddr;
use tokio::{signal, task::JoinSet};
use tracing::info;

use expresso_core::{AppConfig, create_db_pool, create_redis_pool, init_tracing};
use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── Config ─────────────────────────────────────────────────────────────────
    let cfg = AppConfig::from_env()?;
    init_tracing(&cfg.telemetry);

    info!(version = env!("CARGO_PKG_VERSION"), "expresso-mail starting");

    // ── Database ───────────────────────────────────────────────────────────────
    let db    = create_db_pool(&cfg.database).await?;
    let redis = create_redis_pool(&cfg.redis)?;

    let state = AppState::new(cfg.clone(), db, redis);

    // ── Launch servers in parallel ─────────────────────────────────────────────
    let mut set = JoinSet::new();

    // HTTP API
    let http_addr: SocketAddr = format!(
        "{}:{}",
        cfg.server.host, cfg.server.port
    ).parse()?;
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
    set.spawn(async move {
        smtp::serve(smtp_state, smtp_addr).await
    });

    // IMAP (stub)
    let imap_addr: SocketAddr = format!("0.0.0.0:{}", cfg.mail_server.imap_port).parse()?;
    let imap_state = state.clone();
    set.spawn(async move {
        imap::serve(imap_state, imap_addr).await
    });

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

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
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

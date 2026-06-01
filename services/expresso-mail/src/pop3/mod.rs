//! POP3 server (RFC 1939) — plain (port 110) + implicit TLS (port 995, RFC 8314).
//!
//! Architecture:
//!  TcpListener → per-connection task → Pop3Session state machine
//!
//! POP3 is a download-and-delete protocol: a client authenticates, lists the
//! INBOX, retrieves messages, optionally marks them deleted, and on QUIT the
//! server commits the deletions. Unlike IMAP it is single-mailbox (INBOX only),
//! flat (no folders), and has no server-side flag model beyond per-session
//! deletion marks. The line protocol is CRLF-delimited ASCII commands.

mod command;
mod metrics;
mod session;
mod store;

use std::{net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tracing::{error, info, warn};

use crate::state::AppState;

/// Bind and accept plain POP3 connections (port 110).
pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(addr = %addr, "POP3 listener ready");
    metrics::init();

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let st = state.clone();
                metrics::POP3_SESSIONS_TOTAL
                    .with_label_values(&["accepted"])
                    .inc();
                tokio::spawn(async move {
                    match session::handle(stream, st).await {
                        Ok(()) => metrics::POP3_SESSIONS_TOTAL
                            .with_label_values(&["closed"])
                            .inc(),
                        Err(e) => {
                            metrics::POP3_SESSIONS_TOTAL
                                .with_label_values(&["error"])
                                .inc();
                            error!(peer = %peer, error = %e, "POP3 session error");
                        }
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "POP3 accept error");
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// Bind and accept implicit-TLS POP3 connections (port 995, RFC 8314).
/// Only started when mail_server.tls_cert + tls_key are configured.
pub async fn serve_tls(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let (cert, key) = {
        let cfg = state.cfg();
        let c = cfg
            .mail_server
            .tls_cert
            .clone()
            .ok_or_else(|| anyhow::anyhow!("pop3s: mail_server.tls_cert required"))?;
        let k = cfg
            .mail_server
            .tls_key
            .clone()
            .ok_or_else(|| anyhow::anyhow!("pop3s: mail_server.tls_key required"))?;
        (c, k)
    };
    let acceptor = TlsAcceptor::from(Arc::new(crate::imap::load_tls(&cert, &key)?));

    let listener = TcpListener::bind(addr).await?;
    info!(addr = %addr, "POP3S listener ready (implicit TLS)");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let st = state.clone();
                let acc = acceptor.clone();
                metrics::POP3_SESSIONS_TOTAL
                    .with_label_values(&["accepted"])
                    .inc();
                tokio::spawn(async move {
                    match acc.accept(stream).await {
                        Ok(tls_stream) => match session::handle_tls(tls_stream, st).await {
                            Ok(()) => metrics::POP3_SESSIONS_TOTAL
                                .with_label_values(&["closed"])
                                .inc(),
                            Err(e) => {
                                metrics::POP3_SESSIONS_TOTAL
                                    .with_label_values(&["error"])
                                    .inc();
                                error!(peer = %peer, error = %e, "POP3S session error");
                            }
                        },
                        Err(e) => {
                            metrics::POP3_SESSIONS_TOTAL
                                .with_label_values(&["error"])
                                .inc();
                            warn!(peer = %peer, error = %e, "POP3S TLS handshake failed");
                        }
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "POP3S accept error");
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
    }
}

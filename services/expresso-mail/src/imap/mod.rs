//! IMAP4rev1 server (RFC 3501)
//! Core implementation: CAPABILITY, LOGIN, LIST, SELECT, FETCH, STORE, EXPUNGE, CLOSE, LOGOUT, NOOP
//!
//! Architecture:
//!  TcpListener → per-connection task → ImapSession state machine
//!  imap-codec handles framing / serialization; imap-types handles data structures.

mod lockout;
mod metrics;
mod session;

pub use metrics::init as init_metrics;

use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::{error, info};

use crate::state::AppState;

/// Bind and accept IMAP connections, spawning a session per client.
pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(addr = %addr, "IMAP listener ready");
    metrics::init();

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let st = state.clone();
                metrics::IMAP_SESSIONS_TOTAL.with_label_values(&["accepted"]).inc();
                tokio::spawn(async move {
                    match session::handle(stream, st).await {
                        Ok(())  => {
                            metrics::IMAP_SESSIONS_TOTAL.with_label_values(&["closed"]).inc();
                        }
                        Err(e) => {
                            metrics::IMAP_SESSIONS_TOTAL.with_label_values(&["error"]).inc();
                            error!(peer = %peer, error = %e, "IMAP session error");
                        }
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "IMAP accept error");
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
    }
}

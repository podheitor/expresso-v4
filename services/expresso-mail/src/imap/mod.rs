//! IMAP4rev1 server (RFC 3501)
//! Core implementation: CAPABILITY, LOGIN, LIST, SELECT, FETCH, STORE, EXPUNGE, CLOSE, LOGOUT, NOOP
//!
//! Architecture:
//!  TcpListener → per-connection task → ImapSession state machine
//!  imap-codec handles framing / serialization; imap-types handles data structures.

mod session;

use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::{error, info};

use crate::state::AppState;

/// Bind and accept IMAP connections, spawning a session per client.
pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(addr = %addr, "IMAP listener ready");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let st = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = session::handle(stream, st).await {
                        error!(peer = %peer, error = %e, "IMAP session error");
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

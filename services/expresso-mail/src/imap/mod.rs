//! IMAP4rev2 server (RFC 9051)
//! STUB: Full implementation planned for Phase 2 sprint.
//! Clients can currently use the REST API for webmail.
//!
//! Architecture:
//!  TcpListener → per-connection task → ImapSession state machine
//!  ImapSession implements: CAPABILITY, LOGIN, SELECT, FETCH, STORE, COPY, EXPUNGE, LOGOUT
//!  imap-codec handles framing / serialization; imap-types handles data structures.

use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::{info, error};

use crate::state::AppState;

/// STUB: Bind and accept IMAP connections (does not parse protocol yet).
pub async fn serve(_state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(addr = %addr, "IMAP listener ready (stub — closes connection immediately)");

    loop {
        match listener.accept().await {
            Ok((mut stream, _peer)) => {
                tokio::spawn(async move {
                    use tokio::io::AsyncWriteExt;
                    // RFC 9051: server sends greeting, then client initiates
                    let _ = stream.write_all(b"* OK Expresso IMAP4rev2 server (not yet implemented)\r\n").await;
                    // Connection is closed — proper impl in Phase 2
                });
            }
            Err(e) => {
                error!(error = %e, "IMAP accept error");
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
    }
}

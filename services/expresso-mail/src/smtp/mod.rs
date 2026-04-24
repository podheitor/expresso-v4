//! Minimal ESMTP server (RFC 5321) — receives inbound mail on port 25.
//! Parses envelope → calls ingest pipeline → 250 OK or 4xx/5xx.

pub mod session;
pub mod submission;

use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::{info, error};

use crate::state::AppState;

/// Start listening for SMTP connections.
/// Spawns a new task per connection.
pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(addr = %addr, "SMTP listener ready");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = session::handle(stream, peer, state).await {
                        error!(peer = %peer, error = %e, "SMTP session error");
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "SMTP accept error");
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
    }
}

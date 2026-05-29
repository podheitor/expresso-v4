//! IMAP4rev1 server (RFC 3501) — plain (port 143) + implicit TLS (port 993, RFC 8314).
//!
//! Architecture:
//!  TcpListener → per-connection task → ImapSession state machine
//!  imap-codec handles framing / serialization; imap-types handles data structures.

mod lockout;
mod metrics;
mod session;

use std::{net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tracing::{error, info, warn};

use crate::state::AppState;

fn load_tls(cert: &str, key: &str) -> anyhow::Result<rustls::ServerConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert_pem = std::fs::read(cert)?;
    let key_pem = std::fs::read(key)?;
    use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
    use rustls_pki_types::{CertificateDer, PrivateKeyDer};
    let chain: Vec<CertificateDer<'static>> =
        certs(&mut cert_pem.as_slice()).collect::<Result<Vec<_>, _>>()?;
    anyhow::ensure!(!chain.is_empty(), "no certs in {cert}");
    let pkcs8: Vec<_> =
        pkcs8_private_keys(&mut key_pem.as_slice()).collect::<Result<Vec<_>, _>>()?;
    let key: PrivateKeyDer<'static> = if let Some(k) = pkcs8.into_iter().next() {
        PrivateKeyDer::Pkcs8(k)
    } else {
        let rsa: Vec<_> =
            rsa_private_keys(&mut key_pem.as_slice()).collect::<Result<Vec<_>, _>>()?;
        PrivateKeyDer::Pkcs1(
            rsa.into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no private key in {key}"))?,
        )
    };
    Ok(rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(chain, key)?)
}

/// Bind and accept plain IMAP connections (port 143).
pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(addr = %addr, "IMAP listener ready");
    metrics::init();

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let st = state.clone();
                metrics::IMAP_SESSIONS_TOTAL
                    .with_label_values(&["accepted"])
                    .inc();
                tokio::spawn(async move {
                    match session::handle(stream, st).await {
                        Ok(()) => metrics::IMAP_SESSIONS_TOTAL
                            .with_label_values(&["closed"])
                            .inc(),
                        Err(e) => {
                            metrics::IMAP_SESSIONS_TOTAL
                                .with_label_values(&["error"])
                                .inc();
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

/// Bind and accept implicit-TLS IMAP connections (port 993, RFC 8314).
/// Only started when mail_server.tls_cert + tls_key are configured.
pub async fn serve_tls(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let (cert, key) = {
        let cfg = state.cfg();
        let c = cfg
            .mail_server
            .tls_cert
            .clone()
            .ok_or_else(|| anyhow::anyhow!("imaps: mail_server.tls_cert required"))?;
        let k = cfg
            .mail_server
            .tls_key
            .clone()
            .ok_or_else(|| anyhow::anyhow!("imaps: mail_server.tls_key required"))?;
        (c, k)
    };
    let acceptor = TlsAcceptor::from(Arc::new(load_tls(&cert, &key)?));

    let listener = TcpListener::bind(addr).await?;
    info!(addr = %addr, "IMAPS listener ready (implicit TLS)");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let st = state.clone();
                let acc = acceptor.clone();
                metrics::IMAP_SESSIONS_TOTAL
                    .with_label_values(&["accepted"])
                    .inc();
                tokio::spawn(async move {
                    match acc.accept(stream).await {
                        Ok(tls_stream) => match session::handle_tls(tls_stream, st).await {
                            Ok(()) => metrics::IMAP_SESSIONS_TOTAL
                                .with_label_values(&["closed"])
                                .inc(),
                            Err(e) => {
                                metrics::IMAP_SESSIONS_TOTAL
                                    .with_label_values(&["error"])
                                    .inc();
                                error!(peer = %peer, error = %e, "IMAPS session error");
                            }
                        },
                        Err(e) => {
                            metrics::IMAP_SESSIONS_TOTAL
                                .with_label_values(&["error"])
                                .inc();
                            warn!(peer = %peer, error = %e, "IMAPS TLS handshake failed");
                        }
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "IMAPS accept error");
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
    }
}

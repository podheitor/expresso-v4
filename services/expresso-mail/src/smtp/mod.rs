//! ESMTP server (RFC 5321) — inbound port 25 + implicit TLS port 465 (RFC 8314).

pub mod metrics;
pub mod session;
pub mod submission;

use std::{net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tracing::{info, error, warn};

use crate::state::AppState;

fn load_tls(cert: &str, key: &str) -> anyhow::Result<rustls::ServerConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert_pem = std::fs::read(cert)?;
    let key_pem  = std::fs::read(key)?;
    use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
    use rustls_pki_types::{CertificateDer, PrivateKeyDer};
    let chain: Vec<CertificateDer<'static>> = certs(&mut cert_pem.as_slice())
        .collect::<Result<Vec<_>, _>>()?;
    anyhow::ensure!(!chain.is_empty(), "no certs in {cert}");
    let pkcs8: Vec<_> = pkcs8_private_keys(&mut key_pem.as_slice())
        .collect::<Result<Vec<_>, _>>()?;
    let key: PrivateKeyDer<'static> = if let Some(k) = pkcs8.into_iter().next() {
        PrivateKeyDer::Pkcs8(k)
    } else {
        let rsa: Vec<_> = rsa_private_keys(&mut key_pem.as_slice())
            .collect::<Result<Vec<_>, _>>()?;
        PrivateKeyDer::Pkcs1(rsa.into_iter().next()
            .ok_or_else(|| anyhow::anyhow!("no private key in {key}"))?)
    };
    Ok(rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(chain, key)?)
}

/// Start listening for plain SMTP connections (port 25).
pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    metrics::init();
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

/// Start listening for SMTPS connections (port 465, RFC 8314 — implicit TLS).
/// Only called when tls_cert + tls_key are configured.
pub async fn serve_smtps(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let (cert, key) = {
        let cfg = state.cfg();
        let c = cfg.mail_server.tls_cert.clone()
            .ok_or_else(|| anyhow::anyhow!("smtps: mail_server.tls_cert required"))?;
        let k = cfg.mail_server.tls_key.clone()
            .ok_or_else(|| anyhow::anyhow!("smtps: mail_server.tls_key required"))?;
        (c, k)
    };
    let acceptor = TlsAcceptor::from(Arc::new(load_tls(&cert, &key)?));

    let listener = TcpListener::bind(addr).await?;
    info!(addr = %addr, "SMTPS listener ready (implicit TLS)");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let state = state.clone();
                let acc = acceptor.clone();
                tokio::spawn(async move {
                    match acc.accept(stream).await {
                        Ok(tls_stream) => {
                            if let Err(e) = session::handle_smtps(tls_stream, peer, state).await {
                                error!(peer = %peer, error = %e, "SMTPS session error");
                            }
                        }
                        Err(e) => warn!(peer = %peer, error = %e, "SMTPS TLS handshake failed"),
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "SMTPS accept error");
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
    }
}

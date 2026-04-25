//! SMTP Submission (port 587) — STARTTLS + AUTH PLAIN/LOGIN required.
//! Authenticates via Keycloak direct-access grant. DKIM-signs outbound on DATA.
//! Relays signed message to configured relay (or MX lookup — v1 uses relay).

use std::{net::SocketAddr, sync::Arc};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use rustls::ServerConfig;
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, error, info, warn};

use std::sync::OnceLock;
use std::time::Duration;
use expresso_auth_client::{KcBasicAuthenticator, KcBasicConfig, KcBasicError};

use crate::{ingest, state::AppState};

const MAX_MSG_BYTES: usize = 50 * 1024 * 1024; // 50 MiB
const MAX_RCPTS: usize = 100;

/// Start submission listener. Requires TLS cert+key configured in mail_server.
pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let cfg = state.cfg();
    let tls_cert = cfg
        .mail_server
        .tls_cert
        .clone()
        .ok_or_else(|| anyhow::anyhow!("submission: mail_server.tls_cert required"))?;
    let tls_key = cfg
        .mail_server
        .tls_key
        .clone()
        .ok_or_else(|| anyhow::anyhow!("submission: mail_server.tls_key required"))?;

    let tls_cfg = load_tls_config(&tls_cert, &tls_key)?;
    let acceptor = TlsAcceptor::from(Arc::new(tls_cfg));

    let listener = TcpListener::bind(addr).await?;
    info!(addr = %addr, tls_cert = %tls_cert, "submission listener ready (587)");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let state = state.clone();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle(stream, peer, state, acceptor).await {
                        warn!(peer = %peer, error = %e, "submission session error");
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "submission accept error");
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
    }
}

fn load_tls_config(cert_path: &str, key_path: &str) -> anyhow::Result<ServerConfig> {
    // Install default crypto provider (idempotent — ok if already installed)
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cert_pem = std::fs::read(cert_path)?;
    let key_pem = std::fs::read(key_path)?;

    let cert_chain: Vec<CertificateDer<'static>> = certs(&mut cert_pem.as_slice())
        .collect::<Result<Vec<_>, _>>()?;
    if cert_chain.is_empty() {
        anyhow::bail!("no certs found in {}", cert_path);
    }

    // Try PKCS8 first, then RSA
    let pkcs8: Vec<_> = pkcs8_private_keys(&mut key_pem.as_slice())
        .collect::<Result<Vec<_>, _>>()?;
    let key: PrivateKeyDer<'static> = if let Some(k) = pkcs8.into_iter().next() {
        PrivateKeyDer::Pkcs8(k)
    } else {
        let rsa: Vec<_> = rsa_private_keys(&mut key_pem.as_slice())
            .collect::<Result<Vec<_>, _>>()?;
        let k = rsa
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no private key found in {}", key_path))?;
        PrivateKeyDer::Pkcs1(k)
    };

    let cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)?;
    Ok(cfg)
}

#[derive(Default)]
struct Envelope {
    from: Option<String>,
    rcpts: Vec<String>,
    helo: Option<String>,
    authed_user: Option<String>,
}

/// Handle single submission connection. Upgrades to TLS on STARTTLS, then
/// requires AUTH before MAIL FROM.
async fn handle(
    stream: TcpStream,
    peer: SocketAddr,
    state: AppState,
    acceptor: TlsAcceptor,
) -> anyhow::Result<()> {
    let domain = state.cfg().mail_server.domain.clone();

    // Plaintext greeting
    let (reader, writer) = stream.into_split();
    let (done, env) =
        handle_plain(reader, writer, &domain, &state).await?;
    if done {
        return Ok(());
    }
    // STARTTLS path: upgrade
    let stream = env
        .tcp
        .ok_or_else(|| anyhow::anyhow!("STARTTLS requested but stream missing"))?;
    let tls_stream = acceptor.accept(stream).await?;
    info!(peer = %peer, "submission TLS established");
    handle_tls(tls_stream, &domain, &state, env.helo).await
}

struct PreTls {
    tcp: Option<TcpStream>,
    helo: Option<String>,
}

/// Pre-TLS phase: greet, accept EHLO, STARTTLS, QUIT.
/// Returns `(done, PreTls)` — if done=true, connection closed without TLS.
async fn handle_plain(
    reader: tokio::net::tcp::OwnedReadHalf,
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    domain: &str,
    _state: &AppState,
) -> anyhow::Result<(bool, PreTls)> {
    let mut lines = BufReader::new(reader).lines();
    writer
        .write_all(format!("220 {domain} ESMTP Expresso Submission\r\n").as_bytes())
        .await?;

    let mut helo: Option<String> = None;

    while let Some(line) = lines.next_line().await? {
        debug!(line = %line, "submission-plain ←");
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            helo = Some(line[4..].trim().to_string());
            writer
                .write_all(
                    format!(
                        "250-{domain} Hello\r\n250-SIZE {MAX_MSG_BYTES}\r\n250-8BITMIME\r\n250 STARTTLS\r\n"
                    )
                    .as_bytes(),
                )
                .await?;
        } else if upper == "STARTTLS" {
            writer.write_all(b"220 Ready to start TLS\r\n").await?;
            writer.flush().await?;
            // Reunite halves so we can return TcpStream
            let reader = lines.into_inner().into_inner();
            let stream = reader
                .reunite(writer)
                .map_err(|_| anyhow::anyhow!("reunite failed"))?;
            return Ok((false, PreTls { tcp: Some(stream), helo }));
        } else if upper == "QUIT" {
            writer
                .write_all(format!("221 {domain} Bye\r\n").as_bytes())
                .await?;
            return Ok((true, PreTls { tcp: None, helo }));
        } else if upper == "NOOP" {
            writer.write_all(b"250 OK\r\n").await?;
        } else if upper.starts_with("AUTH") || upper.starts_with("MAIL") || upper.starts_with("RCPT") || upper == "DATA" {
            writer
                .write_all(b"530 Must issue a STARTTLS command first\r\n")
                .await?;
        } else {
            writer.write_all(b"500 Command not recognized\r\n").await?;
        }
    }

    Ok((true, PreTls { tcp: None, helo }))
}

/// Post-TLS phase: AUTH required, then MAIL/RCPT/DATA.
async fn handle_tls<S>(
    stream: S,
    domain: &str,
    state: &AppState,
    prev_helo: Option<String>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();

    let mut env = Envelope { helo: prev_helo, ..Default::default() };
    let mut data_mode = false;
    let mut data_buf = String::new();

    while let Some(line) = lines.next_line().await? {
        debug!(line = %line, "submission-tls ←");

        if data_mode {
            if line == "." {
                data_mode = false;
                let raw = data_buf.as_bytes();
                info!(
                    from = ?env.from,
                    rcpts = ?env.rcpts,
                    user = ?env.authed_user,
                    bytes = raw.len(),
                    "submission message received"
                );

                // DKIM-sign outbound if signer configured
                let signed = if let Some(signer) = state.dkim() {
                    match signer.sign(raw) {
                        Ok(s) => s,
                        Err(e) => {
                            warn!(error = %e, "DKIM sign failed — relaying unsigned");
                            data_buf.clone()
                        }
                    }
                } else {
                    data_buf.clone()
                };

                // Ingest (local delivery if recipient matches domain; else relay)
                match ingest::process(
                    state,
                    env.from.as_deref(),
                    &env.rcpts,
                    signed.as_bytes(),
                )
                .await
                {
                    Ok(n) => {
                        writer
                            .write_all(
                                format!("250 OK queued ({n} delivered locally)\r\n").as_bytes(),
                            )
                            .await?;
                    }
                    Err(e) => {
                        error!(error = %e, "submission ingest failed");
                        writer
                            .write_all(b"451 Requested action aborted: local error\r\n")
                            .await?;
                    }
                }

                data_buf.clear();
                env = Envelope {
                    helo: env.helo,
                    authed_user: env.authed_user,
                    ..Default::default()
                };
            } else {
                // Dot-stuffing (RFC 5321 §4.5.2)
                let line = line.strip_prefix('.').unwrap_or(&line);
                if data_buf.len() + line.len() > MAX_MSG_BYTES {
                    writer.write_all(b"552 Message too large\r\n").await?;
                    data_mode = false;
                    data_buf.clear();
                    env = Envelope {
                        helo: env.helo,
                        authed_user: env.authed_user,
                        ..Default::default()
                    };
                } else {
                    data_buf.push_str(line);
                    data_buf.push_str("\r\n");
                }
            }
            continue;
        }

        let upper = line.to_ascii_uppercase();

        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            env.helo = Some(line[4..].trim().to_string());
            writer
                .write_all(
                    format!(
                        "250-{domain} Hello\r\n250-SIZE {MAX_MSG_BYTES}\r\n250-8BITMIME\r\n250-AUTH PLAIN LOGIN\r\n250 OK\r\n"
                    )
                    .as_bytes(),
                )
                .await?;
        } else if upper.starts_with("AUTH PLAIN") {
            // AUTH PLAIN [<base64>] or AUTH PLAIN\r\n followed by base64 line
            let b64 = line[10..].trim();
            let credential = if b64.is_empty() {
                writer.write_all(b"334 \r\n").await?;
                match lines.next_line().await? {
                    Some(l) => l,
                    None => break,
                }
            } else {
                b64.to_string()
            };
            match decode_plain(&credential) {
                Some((user, pass)) => match authenticate(state, &user, &pass).await {
                    Ok(()) => {
                        env.authed_user = Some(user.clone());
                        info!(user = %user, "submission AUTH PLAIN success");
                        writer
                            .write_all(b"235 2.7.0 Authentication successful\r\n")
                            .await?;
                    }
                    Err(e) => {
                        warn!(user = %user, error = %e, "submission AUTH PLAIN fail");
                        writer
                            .write_all(b"535 5.7.8 Authentication credentials invalid\r\n")
                            .await?;
                    }
                },
                None => {
                    writer
                        .write_all(b"501 5.5.2 Cannot decode AUTH PLAIN\r\n")
                        .await?;
                }
            }
        } else if upper.starts_with("AUTH LOGIN") {
            // Prompt for username (base64)
            writer.write_all(b"334 VXNlcm5hbWU6\r\n").await?; // "Username:"
            let user_b64 = match lines.next_line().await? {
                Some(l) => l,
                None => break,
            };
            writer.write_all(b"334 UGFzc3dvcmQ6\r\n").await?; // "Password:"
            let pass_b64 = match lines.next_line().await? {
                Some(l) => l,
                None => break,
            };
            let user = B64.decode(user_b64.trim()).ok().and_then(|b| String::from_utf8(b).ok());
            let pass = B64.decode(pass_b64.trim()).ok().and_then(|b| String::from_utf8(b).ok());
            match (user, pass) {
                (Some(u), Some(p)) => match authenticate(state, &u, &p).await {
                    Ok(()) => {
                        env.authed_user = Some(u.clone());
                        info!(user = %u, "submission AUTH LOGIN success");
                        writer
                            .write_all(b"235 2.7.0 Authentication successful\r\n")
                            .await?;
                    }
                    Err(e) => {
                        warn!(user = %u, error = %e, "submission AUTH LOGIN fail");
                        writer
                            .write_all(b"535 5.7.8 Authentication credentials invalid\r\n")
                            .await?;
                    }
                },
                _ => {
                    writer
                        .write_all(b"501 5.5.2 Cannot decode AUTH LOGIN\r\n")
                        .await?;
                }
            }
        } else if upper.starts_with("MAIL FROM:") {
            let Some(authed) = env.authed_user.as_deref() else {
                writer
                    .write_all(b"530 5.7.0 Authentication required\r\n")
                    .await?;
                continue;
            };
            let from = extract_angle(&line[10..]);
            if !from_matches_authed(&from, authed) {
                warn!(user = %authed, from = %from, "submission MAIL FROM spoof rejected");
                writer
                    .write_all(b"550 5.7.1 MAIL FROM does not match authenticated user\r\n")
                    .await?;
                continue;
            }
            env.from = Some(from);
            env.rcpts.clear();
            writer.write_all(b"250 OK\r\n").await?;
        } else if upper.starts_with("RCPT TO:") {
            if env.authed_user.is_none() {
                writer
                    .write_all(b"530 5.7.0 Authentication required\r\n")
                    .await?;
                continue;
            }
            if env.from.is_none() {
                writer
                    .write_all(b"503 Bad sequence: MAIL first\r\n")
                    .await?;
            } else if env.rcpts.len() >= MAX_RCPTS {
                writer.write_all(b"452 Too many recipients\r\n").await?;
            } else {
                env.rcpts.push(extract_angle(&line[8..]));
                writer.write_all(b"250 OK\r\n").await?;
            }
        } else if upper == "DATA" {
            if env.authed_user.is_none() {
                writer
                    .write_all(b"530 5.7.0 Authentication required\r\n")
                    .await?;
            } else if env.from.is_none() || env.rcpts.is_empty() {
                writer.write_all(b"503 Bad sequence\r\n").await?;
            } else {
                writer
                    .write_all(b"354 Start input; end with <CRLF>.<CRLF>\r\n")
                    .await?;
                data_mode = true;
            }
        } else if upper == "RSET" {
            env = Envelope {
                helo: env.helo,
                authed_user: env.authed_user,
                ..Default::default()
            };
            writer.write_all(b"250 OK\r\n").await?;
        } else if upper == "NOOP" {
            writer.write_all(b"250 OK\r\n").await?;
        } else if upper == "QUIT" {
            writer
                .write_all(format!("221 {domain} Bye\r\n").as_bytes())
                .await?;
            break;
        } else {
            warn!(cmd = %line, "unknown submission command");
            writer.write_all(b"500 Command not recognized\r\n").await?;
        }
    }

    Ok(())
}

fn extract_angle(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('<') && s.ends_with('>') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// True if MAIL FROM matches the authenticated user (case-insensitive,
/// whitespace-trimmed). Empty MAIL FROM (`<>` bounce) is rejected — submission
/// clients should never send bounces. The authed credential may be a bare
/// username (Keycloak-side) rather than a full email; we reject when authed
/// has no `@` so we never accept a partial-string match.
fn from_matches_authed(from: &str, authed: &str) -> bool {
    let f = from.trim();
    let a = authed.trim();
    if f.is_empty() || a.is_empty() { return false; }
    if !a.contains('@') { return false; }
    f.eq_ignore_ascii_case(a)
}

/// Decode AUTH PLAIN credential: base64("\0user\0pass")
fn decode_plain(b64: &str) -> Option<(String, String)> {
    let decoded = B64.decode(b64.trim()).ok()?;
    let s = String::from_utf8(decoded).ok()?;
    let mut parts = s.splitn(3, '\0');
    let _authzid = parts.next()?; // usually empty
    let user = parts.next()?.to_string();
    let pass = parts.next()?.to_string();
    if user.is_empty() || pass.is_empty() {
        return None;
    }
    Some((user, pass))
}

/// Authenticate user+pass via Keycloak direct-access grant.
/// Uses env AUTH__SUBMISSION_REALM + AUTH__SUBMISSION_CLIENT_ID +
/// AUTH__SUBMISSION_CLIENT_SECRET + AUTH__KC_URL.
///
/// Backed por `KcBasicAuthenticator` (lib expresso-auth-client) — ganha
/// cache de credenciais (TTL 60s, dedup PROPFIND-style burst) + lockout
/// per-username (10 fails/60s → 5min). Mesmo modelo do CalDAV/IMAP
/// fechado no sprint #105; até então o submission re-batia no KC a
/// cada AUTH PLAIN sem freio nenhum (brute-force aberto).
///
/// Single global authenticator: o lockout vale across-connections, que
/// é o que importa contra distributed brute-force vindo de N MUAs
/// numa botnet usando o mesmo username.
static KC_AUTH: OnceLock<KcBasicAuthenticator> = OnceLock::new();

fn kc_authenticator() -> anyhow::Result<&'static KcBasicAuthenticator> {
    if let Some(a) = KC_AUTH.get() {
        return Ok(a);
    }
    let url = std::env::var("AUTH__KC_URL")
        .unwrap_or_else(|_| "http://expresso-keycloak:8080".to_string());
    let realm = std::env::var("AUTH__SUBMISSION_REALM")
        .map_err(|_| anyhow::anyhow!("AUTH__SUBMISSION_REALM env not set"))?;
    let client_id = std::env::var("AUTH__SUBMISSION_CLIENT_ID")
        .unwrap_or_else(|_| "expresso-dav".to_string());
    let client_secret = std::env::var("AUTH__SUBMISSION_CLIENT_SECRET").ok();
    let cfg = KcBasicConfig {
        url, realm, client_id, client_secret,
        cache_ttl:        Duration::from_secs(60),
        http_timeout:     Duration::from_secs(10),
        max_failures:     10,
        failure_window:   Duration::from_secs(60),
        lockout_duration: Duration::from_secs(5 * 60),
    };
    // Race entre threads: `set` falha quando outro já populou — pegamos
    // o valor existente em vez de retornar erro.
    let _ = KC_AUTH.set(KcBasicAuthenticator::new(cfg));
    Ok(KC_AUTH.get().expect("KC_AUTH set above"))
}

async fn authenticate(state: &AppState, user: &str, pass: &str) -> anyhow::Result<()> {
    let _ = state; // reserved for future per-tenant resolver
    let a = kc_authenticator()?;
    match a.authenticate(user, pass).await {
        Ok(_) => Ok(()),
        Err(KcBasicError::InvalidCredentials) =>
            anyhow::bail!("kc auth rejected: invalid credentials"),
        Err(KcBasicError::Unreachable(s)) =>
            anyhow::bail!("kc unreachable: {s}"),
        Err(KcBasicError::Upstream(s)) =>
            anyhow::bail!("kc upstream: {s}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_plain_ok() {
        let cred = B64.encode(b"\0alice@example.com\0secret123");
        let (u, p) = decode_plain(&cred).unwrap();
        assert_eq!(u, "alice@example.com");
        assert_eq!(p, "secret123");
    }

    #[test]
    fn decode_plain_with_authzid() {
        let cred = B64.encode(b"admin\0alice@example.com\0pw");
        let (u, p) = decode_plain(&cred).unwrap();
        assert_eq!(u, "alice@example.com");
        assert_eq!(p, "pw");
    }

    #[test]
    fn decode_plain_rejects_empty_user() {
        let cred = B64.encode(b"\0\0pass");
        assert!(decode_plain(&cred).is_none());
    }

    #[test]
    fn decode_plain_rejects_malformed() {
        assert!(decode_plain("not-base64-!@#").is_none());
        let no_nulls = B64.encode(b"no-nulls");
        assert!(decode_plain(&no_nulls).is_none());
    }

    #[test]
    fn extract_angle_variants() {
        assert_eq!(extract_angle("<a@b.com>"), "a@b.com");
        assert_eq!(extract_angle("a@b.com"), "a@b.com");
        assert_eq!(extract_angle("  <a@b.com>  "), "a@b.com");
    }
}

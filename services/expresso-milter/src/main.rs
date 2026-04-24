//! expresso-milter — Postfix sidecar (indymilter 0.3)
//!
//! Dual-path design:
//!   * **Inbound** (no AUTH): buffer headers+body → verify SPF/DKIM/DMARC →
//!     add `Authentication-Results` header at EOM.
//!   * **Outbound** (AUTH submission, macro `{auth_authen}` set): reassemble
//!     message → sign via DKIM → `insert_header(0, "DKIM-Signature", ...)`.
//!
//! Env:
//!   MILTER_ADDR     listen (default "0.0.0.0:8891")
//!   MAIL_DOMAIN     auth-serv-id + DKIM domain
//!   DKIM_SELECTOR   selector name (unset = no outbound sign)
//!   DKIM_KEY_PATH   RSA private key PEM path

use std::{env, ffi::{CStr, CString}, net::{IpAddr, SocketAddr}, sync::Arc};

use bytes::Bytes;
use indymilter::{
    Actions, Callbacks, ContextActions, MacroStage, NegotiateContext, SocketInfo, Status,
};
use tracing::{debug, info, warn};

use expresso_mail_auth::{DkimSignerState, verify_inbound, MAIL_AUTH_ACTIONS_TOTAL};

/// Per-session accumulator.
#[derive(Default)]
struct Session {
    peer_ip: Option<IpAddr>,
    helo: String,
    mail_from: String,
    headers: Vec<u8>,
    body: Vec<u8>,
}

impl Session {
    fn as_raw(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.headers.len() + 2 + self.body.len());
        out.extend_from_slice(&self.headers);
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    let addr: SocketAddr = env::var("MILTER_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8891".into())
        .parse()?;
    let mail_domain = Arc::new(env::var("MAIL_DOMAIN").unwrap_or_else(|_| "localhost".into()));

    // Load DKIM signer if configured
    let dkim_signer: Option<Arc<DkimSignerState>> = match (
        env::var("DKIM_SELECTOR").ok().filter(|s| !s.is_empty()),
        env::var("DKIM_KEY_PATH").ok().filter(|s| !s.is_empty()),
    ) {
        (Some(sel), Some(path)) => match DkimSignerState::from_pem_file(&mail_domain, &sel, &path) {
            Ok(s) => Some(Arc::new(s)),
            Err(e) => {
                warn!(error = %e, "DKIM signer load failed — outbound will NOT be signed");
                None
            }
        },
        _ => {
            info!("DKIM signer not configured — outbound will NOT be signed");
            None
        }
    };

    info!(%addr, domain = %mail_domain, dkim = dkim_signer.is_some(), "expresso-milter starting");

    let callbacks: Callbacks<Session> = Callbacks::new()
        .on_negotiate(|cx: &mut NegotiateContext<Session>, _actions, _opts| {
            Box::pin(async move {
                cx.requested_actions |= Actions::ADD_HEADER;
                let mail_macros = CString::new("{auth_authen}").unwrap();
                cx.requested_macros.insert(MacroStage::Mail, mail_macros);
                Status::Continue
            })
        })
        .on_connect(|cx, _hostname, socket_info| Box::pin(async move {
            let peer_ip = match socket_info {
                SocketInfo::Inet(a) => Some(a.ip()),
                _ => None,
            };
            cx.data = Some(Session { peer_ip, ..Session::default() });
            Status::Continue
        }))
        .on_helo(|cx, hostname| Box::pin(async move {
            if let Some(s) = cx.data.as_mut() {
                s.helo = hostname.to_string_lossy().into_owned();
            }
            Status::Continue
        }))
        .on_mail(|cx, args| Box::pin(async move {
            if let Some(s) = cx.data.as_mut() {
                if let Some(first) = args.first() {
                    s.mail_from = first.to_string_lossy()
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .to_owned();
                }
            }
            Status::Continue
        }))
        .on_header(|cx, name, value| Box::pin(async move {
            if let Some(s) = cx.data.as_mut() {
                s.headers.extend_from_slice(name.as_bytes());
                s.headers.extend_from_slice(b": ");
                s.headers.extend_from_slice(value.as_bytes());
                s.headers.extend_from_slice(b"\r\n");
            }
            Status::Continue
        }))
        .on_body(|cx, chunk: Bytes| Box::pin(async move {
            if let Some(s) = cx.data.as_mut() {
                s.body.extend_from_slice(&chunk);
            }
            Status::Continue
        }))
        .on_eom({
            let domain = mail_domain.clone();
            let signer = dkim_signer.clone();
            move |cx| {
                let domain = domain.clone();
                let signer = signer.clone();
                Box::pin(async move {
                    let session = match cx.data.as_ref() {
                        Some(s) => s,
                        None => return Status::Continue,
                    };

                    // Check AUTH — {auth_authen} macro present & non-empty ⇒ outbound
                    let auth_authen: Option<String> = cx.macros
                        .get(CStr::from_bytes_with_nul(b"{auth_authen}\0").unwrap())
                        .map(|v| v.to_string_lossy().into_owned())
                        .filter(|v| !v.is_empty());

                    let raw = session.as_raw();

                    if let Some(user) = auth_authen {
                        // OUTBOUND path — sign DKIM if signer configured
                        debug!(user, "outbound (AUTH session)");
                        if let Some(signer) = signer.as_ref() {
                            match signer.sign(&raw) {
                                Ok(sig_header) => {
                                    // sig_header format: "DKIM-Signature: v=1; ...\r\n"
                                    // Split name + value for insert_header
                                    if let Some((name, value)) = parse_header_line(&sig_header) {
                                        let name_c  = CString::new(name).unwrap();
                                        let value_c = match CString::new(value) {
                                            Ok(v) => v,
                                            Err(e) => {
                                                warn!(error = %e, "DKIM value had NUL");
                                                return Status::Continue;
                                            }
                                        };
                                        if let Err(e) = cx.actions.insert_header(0, name_c, value_c).await {
                                            warn!(error = %e, "insert DKIM header failed");
                                        } else {
                                            info!(user, "outbound DKIM signed");
                                        }
                                    } else {
                                        warn!("could not parse DKIM-Signature header line");
                                    }
                                }
                                Err(e) => warn!(error = %e, "DKIM sign failed"),
                            }
                        } else {
                            debug!("no signer configured — skipping outbound sign");
                        }
                    } else {
                        // INBOUND path — verify + inject A-R
                        let ip = match session.peer_ip {
                            Some(ip) => ip,
                            None => {
                                debug!("no peer IP — skipping verify");
                                return Status::Continue;
                            }
                        };
                        let auth = verify_inbound(ip, &session.helo, &session.mail_from, &domain, &raw).await;
                        info!(spf = %auth.spf, dkim = %auth.dkim, dmarc = %auth.dmarc,
                              policy = ?auth.dmarc_policy, "inbound auth");
                        let name  = CString::new("Authentication-Results").unwrap();
                        let value = match CString::new(auth.to_value(&domain)) {
                            Ok(v) => v,
                            Err(e) => {
                                warn!(error = %e, "A-R value had NUL");
                                return Status::Continue;
                            }
                        };
                        if let Err(e) = cx.actions.add_header(name, value).await {
                            warn!(error = %e, "add_header failed");
                        }

                        // Received-SPF trace header (RFC 7208 §9.1).
                        let rspf_name = CString::new("Received-SPF").unwrap();
                        match CString::new(auth.to_received_spf(&domain)) {
                            Ok(v) => {
                                if let Err(e) = cx.actions.add_header(rspf_name, v).await {
                                    warn!(error = %e, "add Received-SPF failed");
                                }
                            }
                            Err(e) => warn!(error = %e, "Received-SPF value had NUL"),
                        }

                        // DMARC policy enforcement.
                        if auth.should_reject() {
                            warn!(from = %session.mail_from, "DMARC fail + p=reject — rejecting");
                            MAIL_AUTH_ACTIONS_TOTAL.with_label_values(&["reject"]).inc();
                            return Status::Reject;
                        }
                        if auth.should_quarantine() {
                            warn!(from = %session.mail_from, "DMARC fail + p=quarantine — holding");
                            let reason = CString::new("DMARC fail (p=quarantine)").unwrap();
                            if let Err(e) = cx.actions.quarantine(reason).await {
                                warn!(error = %e, "quarantine action failed");
                            }
                            MAIL_AUTH_ACTIONS_TOTAL.with_label_values(&["quarantine"]).inc();
                            return Status::Continue;
                        }
                        MAIL_AUTH_ACTIONS_TOTAL.with_label_values(&["accept"]).inc();
                    }
                    Status::Continue
                })
            }
        });

    // Spawn metrics/health HTTP server on secondary port
    let metrics_addr: SocketAddr = env::var("METRICS_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:9091".into())
        .parse()?;
    tokio::spawn(async move {
        use axum::{routing::get, Router, Json};
        let app: Router = Router::new()
            .route("/health", get(|| async { Json(serde_json::json!({"service":"expresso-milter","status":"ok"})) }))
            .route("/ready",  get(|| async { Json(serde_json::json!({"service":"expresso-milter","status":"ready"})) }))
            .merge(expresso_observability::metrics_router());
        match tokio::net::TcpListener::bind(metrics_addr).await {
            Ok(lst) => {
                info!(%metrics_addr, "metrics HTTP listener ready");
                if let Err(e) = axum::serve(lst, app).await {
                    warn!(error=%e, "metrics serve failed");
                }
            }
            Err(e) => warn!(error=%e, %metrics_addr, "metrics bind failed"),
        }
    });

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "milter listener ready");
    let shutdown = async { let _ = tokio::signal::ctrl_c().await; };
    indymilter::run(listener, callbacks, Default::default(), shutdown).await?;
    Ok(())
}

/// Parse a "Name: value\r\n" line → (name, value_trimmed).
fn parse_header_line(line: &str) -> Option<(String, String)> {
    let line = line.trim_end_matches("\r\n").trim_end_matches('\n');
    let (name, rest) = line.split_once(':')?;
    Some((name.trim().to_owned(), rest.trim_start().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::parse_header_line;

    #[test]
    fn parse_header_basic() {
        let (n, v) = parse_header_line("DKIM-Signature: v=1; a=rsa-sha256\r\n").unwrap();
        assert_eq!(n, "DKIM-Signature");
        assert_eq!(v, "v=1; a=rsa-sha256");
    }

    #[test]
    fn parse_header_no_crlf() {
        let (n, v) = parse_header_line("X-Foo: bar baz").unwrap();
        assert_eq!(n, "X-Foo");
        assert_eq!(v, "bar baz");
    }
}

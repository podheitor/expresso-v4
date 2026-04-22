//! expresso-milter — Postfix sidecar (indymilter 0.3)
//!
//! Inbound path: buffers headers + body across callbacks, reassembles raw
//! RFC 5322 message at EOM, runs SPF/DKIM/DMARC via `expresso-mail-auth`,
//! injects `Authentication-Results` header.
//!
//! Outbound DKIM signing (AUTH submission) = TODO — requires capturing the
//! final headers+body as Postfix will send them downstream.
//!
//! Env:
//!   MILTER_ADDR   listen addr (default "0.0.0.0:8891")
//!   MAIL_DOMAIN   auth-serv-id in Authentication-Results

use std::{env, ffi::CString, net::{IpAddr, SocketAddr}, sync::Arc};

use bytes::Bytes;
use indymilter::{
    Actions, Callbacks, ContextActions, NegotiateContext, SocketInfo, Status,
};
use tracing::{debug, info, warn};

use expresso_mail_auth::verify_inbound;

/// Per-session accumulator.
#[derive(Default)]
struct Session {
    peer_ip: Option<IpAddr>,
    helo: String,
    mail_from: String,
    /// Raw headers bytes (with CRLF separators).
    headers: Vec<u8>,
    /// Raw body bytes.
    body: Vec<u8>,
}

impl Session {
    /// Reconstruct message as bytes: headers + CRLF + body.
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

    info!(%addr, domain = %mail_domain, "expresso-milter starting");

    let domain_neg = mail_domain.clone();
    let domain_eom = mail_domain.clone();

    let callbacks: Callbacks<Session> = Callbacks::new()
        .on_negotiate(move |cx: &mut NegotiateContext<Session>, _actions, _opts| {
            let _ = &domain_neg;
            Box::pin(async move {
                cx.requested_actions |= Actions::ADD_HEADER;
                Status::Continue
            })
        })
        .on_connect(|cx, _hostname, socket_info| Box::pin(async move {
            let mut s = Session::default();
            s.peer_ip = match socket_info {
                SocketInfo::Inet(a) => Some(a.ip()),
                _ => None,
            };
            cx.data = Some(s);
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
        .on_eom(move |cx| {
            let domain = domain_eom.clone();
            Box::pin(async move {
                let session = match cx.data.as_ref() {
                    Some(s) => s,
                    None => return Status::Continue,
                };
                let ip = match session.peer_ip {
                    Some(ip) => ip,
                    None => {
                        debug!("no peer IP — skipping verify");
                        return Status::Continue;
                    }
                };
                let raw = session.as_raw();
                let auth = verify_inbound(ip, &session.helo, &session.mail_from, &domain, &raw).await;
                info!(spf = %auth.spf, dkim = %auth.dkim, dmarc = %auth.dmarc, "auth results");

                let name = CString::new("Authentication-Results").unwrap();
                let value = match CString::new(auth.to_value(&domain)) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "A-R value contained NUL");
                        return Status::Continue;
                    }
                };
                if let Err(e) = cx.actions.add_header(name, value).await {
                    warn!(error = %e, "add_header failed");
                }
                Status::Continue
            })
        });

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "milter listener ready");
    let shutdown = async { let _ = tokio::signal::ctrl_c().await; };
    indymilter::run(listener, callbacks, Default::default(), shutdown).await?;
    Ok(())
}

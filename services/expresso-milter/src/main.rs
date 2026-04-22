//! expresso-milter — Postfix sidecar (Rust, indymilter 0.3)
//!
//! Scaffolding: negotiates ADD_HEADER, on end-of-message injects an
//! `Authentication-Results` header. Real verification/signing = TODO.
//!
//! Env:
//!   MILTER_ADDR     listen addr (default "0.0.0.0:8891")
//!   MAIL_DOMAIN     signing + auth-serv-id domain
//!   DKIM_SELECTOR   (future) RSA selector
//!   DKIM_KEY_PATH   (future) RSA key PEM path
//!
//! Scope note: inbound SPF/DKIM/DMARC verification is ALSO implemented
//! in expresso-mail itself (app ESMTP listener). This milter is used only
//! when Postfix fronts the MTA. See docs/MTA-SETUP.md.

use std::{env, ffi::CString, net::SocketAddr};

use indymilter::{Actions, Callbacks, ContextActions, NegotiateContext, Status};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    let addr: SocketAddr = env::var("MILTER_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8891".into())
        .parse()?;
    let mail_domain = env::var("MAIL_DOMAIN").unwrap_or_default();
    let dkim_key_path = env::var("DKIM_KEY_PATH").unwrap_or_default();

    info!(
        %addr, %mail_domain,
        dkim_configured = !dkim_key_path.is_empty(),
        "expresso-milter starting (scaffold mode)"
    );

    let callbacks: Callbacks<()> = Callbacks::new()
        .on_negotiate(|cx: &mut NegotiateContext<()>, _actions, _opts| {
            Box::pin(async move {
                cx.requested_actions |= Actions::ADD_HEADER;
                Status::Continue
            })
        })
        .on_eom(move |cx| {
            let domain = mail_domain.clone();
            Box::pin(async move {
                let name  = CString::new("Authentication-Results").unwrap();
                let value = CString::new(format!("{}; none (expresso-milter scaffold)", domain)).unwrap();
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

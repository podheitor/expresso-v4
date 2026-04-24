//! GET /auth/login → 302 to Keycloak authorization_endpoint.
//!
//! Multi-tenant: resolve realm from Host header → use that realm's provider.
//! Single-tenant fallback: uses static `RpConfig`.

use std::{sync::Arc, time::Instant};

use axum::{
    extract::{Query, State},
    http::{header::HOST, HeaderMap},
    response::Redirect,
};
use serde::Deserialize;

use crate::error::{Result, RpError};
use crate::oidc::pkce::{challenge_s256, generate_verifier, random_token};
use crate::state::{AppState, PendingLogin};

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    pub redirect_uri: Option<String>,
}

pub async fn login(
    State(app): State<Arc<AppState>>,
    headers:    HeaderMap,
    Query(q):   Query<LoginQuery>,
) -> Result<Redirect> {
    let host = headers.get(HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_string();

    // Resolve (authorization_endpoint, realm, redirect_uri).
    let (auth_ep, realm_opt, redirect_uri) = if app.is_multi() {
        match app.realm_for_host(&host) {
            Some(realm) => {
                let cache = app.multi_provider.as_ref().expect("is_multi");
                let prov = cache.get_or_fetch(&realm).await?;
                let ru = app.redirect_uri_for_host(&host);
                (prov.authorization_endpoint.clone(), Some(realm), ru)
            }
            None => {
                // Host not mapped → fall back to single-realm default.
                (app.provider.authorization_endpoint.clone(), None, app.cfg.redirect_uri.clone())
            }
        }
    } else {
        (app.provider.authorization_endpoint.clone(), None, app.cfg.redirect_uri.clone())
    };

    let verifier  = generate_verifier();
    let challenge = challenge_s256(&verifier);
    let state_tok = random_token();
    let nonce     = random_token();

    app.insert_pending(
        state_tok.clone(),
        PendingLogin {
            code_verifier:       verifier,
            post_login_redirect: q.redirect_uri,
            redirect_uri:        redirect_uri.clone(),
            realm:               realm_opt.clone(),
            expires_at:          Instant::now() + app.state_ttl(),
        },
    ).await;

    let mut url = url::Url::parse(&auth_ep)
        .map_err(|e| RpError::Discovery(format!("invalid authorization_endpoint: {e}")))?;
    url.query_pairs_mut()
        .append_pair("response_type",         "code")
        .append_pair("client_id",             &app.cfg.client_id)
        .append_pair("redirect_uri",          &redirect_uri)
        .append_pair("scope",                 "openid profile email")
        .append_pair("state",                 &state_tok)
        .append_pair("nonce",                 &nonce)
        .append_pair("code_challenge",        &challenge)
        .append_pair("code_challenge_method", "S256");

    tracing::info!(
        target: "audit",
        event = "auth.login.start",
        state = %state_tok,
        host  = %host,
        realm = ?realm_opt,
        "login initiated"
    );
    Ok(Redirect::to(url.as_str()))
}

//! GET /auth/login → 302 to Keycloak authorization_endpoint.

use std::{sync::Arc, time::Instant};

use axum::{
    extract::{Query, State},
    response::Redirect,
};
use serde::Deserialize;

use crate::error::Result;
use crate::oidc::pkce::{challenge_s256, generate_verifier, random_token};
use crate::state::{AppState, PendingLogin};

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    pub redirect_uri: Option<String>,
}

pub async fn login(
    State(app): State<Arc<AppState>>,
    Query(q):   Query<LoginQuery>,
) -> Result<Redirect> {
    let verifier  = generate_verifier();
    let challenge = challenge_s256(&verifier);
    let state_tok = random_token();
    let nonce     = random_token();

    app.insert_pending(
        state_tok.clone(),
        PendingLogin {
            code_verifier: verifier,
            post_login_redirect: q.redirect_uri,
            expires_at:    Instant::now() + app.state_ttl(),
        },
    ).await;

    let mut url = url::Url::parse(&app.provider.authorization_endpoint)
        .expect("authorization_endpoint is valid URL");
    url.query_pairs_mut()
        .append_pair("response_type",         "code")
        .append_pair("client_id",             &app.cfg.client_id)
        .append_pair("redirect_uri",          &app.cfg.redirect_uri)
        .append_pair("scope",                 "openid profile email")
        .append_pair("state",                 &state_tok)
        .append_pair("nonce",                 &nonce)
        .append_pair("code_challenge",        &challenge)
        .append_pair("code_challenge_method", "S256");

    tracing::info!(
        target: "audit",
        event = "auth.login.start",
        state = %state_tok,
        "login initiated"
    );
    Ok(Redirect::to(url.as_str()))
}

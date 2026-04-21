//! GET /auth/callback → token exchange, return TokenResponse + post-login URI.

use std::sync::Arc;

use axum::{extract::{Query, State}, Json};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::error::{Result, RpError};
use crate::oidc::tokens::{AuthCodeRequest, TokenResponse};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code:  Option<String>,
    pub state: Option<String>,
    // Error params from IdP redirect
    pub error:             Option<String>,
    pub error_description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CallbackResponse {
    #[serde(flatten)]
    pub tokens: TokenResponse,
    /// Optional redirect target the SPA should navigate to post-login.
    pub post_login_redirect: Option<String>,
}

pub async fn callback(
    State(app): State<Arc<AppState>>,
    Query(q):   Query<CallbackQuery>,
) -> Result<Json<CallbackResponse>> {
    if let Some(err) = q.error {
        warn!(%err, desc = ?q.error_description, "IdP returned error");
        return Err(RpError::TokenExchange(q.error_description.unwrap_or(err)));
    }
    let code  = q.code.ok_or(RpError::BadRequest("missing code"))?;
    let state = q.state.ok_or(RpError::BadRequest("missing state"))?;

    let pending = app.take_pending(&state).await
        .ok_or(RpError::StateNotFound)?;

    let form = AuthCodeRequest {
        grant_type:    "authorization_code",
        code:          &code,
        redirect_uri:  &app.cfg.redirect_uri,
        client_id:     &app.cfg.client_id,
        code_verifier: &pending.code_verifier,
    };

    let resp = app.http
        .post(&app.provider.token_endpoint)
        .form(&form)
        .send()
        .await?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(RpError::TokenExchange(body));
    }
    let tokens: TokenResponse = resp.json().await
        .map_err(|e| RpError::TokenExchange(e.to_string()))?;

    // Validate access_token signature (defense-in-depth: ensures IdP honored
    // our audience + the RP's issuer config matches tokens it's handing out).
    // We intentionally do NOT surface the AuthContext from here — callers hit
    // /auth/me for that. Validation errors surface as 401.
    let _ctx = app.validator.validate(&tokens.access_token).await?;

    Ok(Json(CallbackResponse {
        tokens,
        post_login_redirect: pending.post_login_redirect,
    }))
}

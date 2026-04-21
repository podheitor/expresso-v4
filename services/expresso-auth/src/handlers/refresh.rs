//! POST /auth/refresh → new TokenResponse.
//!
//! Accepts refresh_token from JSON body OR `expresso_rt` cookie. On success
//! re-issues both cookies (for browser) and returns JSON.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header::{COOKIE, SET_COOKIE}, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

use expresso_auth_client::ACCESS_TOKEN_COOKIE;

use crate::error::{Result, RpError};
use crate::oidc::tokens::{RefreshRequest, TokenResponse};
use crate::state::AppState;

const REFRESH_TOKEN_COOKIE: &str = "expresso_rt";

#[derive(Debug, Deserialize, Default)]
pub struct RefreshBody {
    pub refresh_token: Option<String>,
}

pub async fn refresh(
    State(app):       State<Arc<AppState>>,
    headers:          HeaderMap,
    body:             Option<Json<RefreshBody>>,
) -> Result<Response> {
    let from_body = body.and_then(|Json(b)| b.refresh_token);
    let token = from_body
        .or_else(|| extract_cookie(&headers, REFRESH_TOKEN_COOKIE))
        .ok_or(RpError::BadRequest("missing refresh_token"))?;

    let form = RefreshRequest {
        grant_type:    "refresh_token",
        refresh_token: &token,
        client_id:     &app.cfg.client_id,
    };
    let resp = app.http
        .post(&app.provider.token_endpoint)
        .form(&form)
        .send()
        .await?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(RpError::Refresh(body));
    }
    let tokens: TokenResponse = resp.json().await
        .map_err(|e| RpError::Refresh(e.to_string()))?;

    tracing::info!(
        target: "audit",
        event = "auth.token.refreshed",
        "access token refreshed"
    );

    let mut resp = (StatusCode::OK, Json(&tokens)).into_response();
    let secure = std::env::var("AUTH_RP__COOKIE_SECURE").ok().as_deref() == Some("1");
    let sec    = if secure { "; Secure" } else { "" };
    let at = format!(
        "{ACCESS_TOKEN_COOKIE}={}; HttpOnly; Path=/; SameSite=Lax; Max-Age={}{sec}",
        tokens.access_token, tokens.expires_in.max(0)
    );
    resp.headers_mut().append(SET_COOKIE, at.parse().unwrap());
    if let Some(rt) = tokens.refresh_token.as_deref() {
        let max = tokens.refresh_expires_in.unwrap_or(86_400).max(0);
        let c = format!(
            "{REFRESH_TOKEN_COOKIE}={rt}; HttpOnly; Path=/auth/refresh; SameSite=Lax; Max-Age={max}{sec}"
        );
        resp.headers_mut().append(SET_COOKIE, c.parse().unwrap());
    }
    Ok(resp)
}

fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    for hv in headers.get_all(COOKIE).iter() {
        let s = match hv.to_str() { Ok(v) => v, Err(_) => continue };
        for pair in s.split(';') {
            let pair = pair.trim();
            if let Some((k, v)) = pair.split_once('=') {
                if k.trim() == name {
                    let v = v.trim();
                    if !v.is_empty() { return Some(v.to_string()); }
                }
            }
        }
    }
    None
}

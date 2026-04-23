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
use expresso_core::audit::{record_async, AuditEntry};

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
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        if let Some(pool) = app.pool.as_ref() {
            record_async(pool.clone(), AuditEntry {
                tenant_id:   None,
                actor_sub:   None,
                actor_email: None,
                actor_roles: vec![],
                action:      "auth.token.refresh.failure".into(),
                target_type: Some("refresh_token".into()),
                target_id:   None,
                http_method: Some("POST".into()),
                http_path:   Some("/auth/refresh".into()),
                status_code: Some(status.as_u16() as i16),
                metadata:    serde_json::json!({ "upstream_error": body_text.chars().take(500).collect::<String>() }),
            });
        }
        return Err(RpError::Refresh(body_text));
    }
    let tokens: TokenResponse = resp.json().await
        .map_err(|e| RpError::Refresh(e.to_string()))?;

    // Sample-audit successful refreshes (10% of traffic) — errors already audited above.
    if rand::random::<u8>() < 26 {
        if let Some(pool) = app.pool.as_ref() {
            // Best-effort: peek at the fresh access_token to recover identity for the audit row.
            let (sub, email, tenant, roles) = match app.validator.validate(&tokens.access_token).await {
                Ok(ctx) => (
                    Some(ctx.user_id.to_string()),
                    Some(ctx.email),
                    Some(ctx.tenant_id),
                    ctx.roles,
                ),
                Err(_) => (None, None, None, vec![]),
            };
            record_async(pool.clone(), AuditEntry {
                tenant_id:   tenant,
                actor_sub:   sub,
                actor_email: email,
                actor_roles: roles,
                action:      "auth.token.refresh.success".into(),
                target_type: Some("refresh_token".into()),
                target_id:   None,
                http_method: Some("POST".into()),
                http_path:   Some("/auth/refresh".into()),
                status_code: Some(200),
                metadata:    serde_json::json!({ "sampled": true, "rate": 0.1 }),
            });
        }
    }
    tracing::info!(target: "audit", event = "auth.token.refreshed", "access token refreshed");

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

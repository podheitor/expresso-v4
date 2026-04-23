//! GET /auth/logout → clear session cookies + redirect to IdP end_session.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{header::SET_COOKIE, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;

use axum::http::{header::COOKIE, HeaderMap};
use expresso_auth_client::ACCESS_TOKEN_COOKIE;
use expresso_core::audit::{self, AuditEntry};

use crate::error::{Result, RpError};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct LogoutQuery {
    pub id_token_hint: Option<String>,
}

pub async fn logout(
    State(app): State<Arc<AppState>>,
    headers:    HeaderMap,
    Query(q):   Query<LogoutQuery>,
) -> Result<Response> {
    // Best-effort audit: extract access_token from cookie → validate → record.
    // Errors here are non-fatal: logout must always succeed from user POV.
    if let Some(pool) = app.pool.as_ref() {
        let token = headers.get(COOKIE).and_then(|h| h.to_str().ok()).and_then(|c| {
            c.split(';').map(str::trim).find_map(|kv| {
                let (k, v) = kv.split_once('=')?;
                if k == ACCESS_TOKEN_COOKIE { Some(v.to_string()) } else { None }
            })
        });
        if let Some(tok) = token {
            if let Ok(ctx) = app.validator.validate(&tok).await {
                let entry = AuditEntry {
                    tenant_id:   Some(ctx.tenant_id),
                    actor_sub:   Some(ctx.user_id.to_string()),
                    actor_email: Some(ctx.email.clone()),
                    actor_roles: ctx.roles.clone(),
                    action:      "auth.logout".into(),
                    target_type: Some("user".into()),
                    target_id:   Some(ctx.user_id.to_string()),
                    http_method: Some("GET".into()),
                    http_path:   Some("/auth/logout".into()),
                    status_code: Some(303),
                    metadata:    serde_json::json!({}),
                };
                audit::record_async(pool.clone(), entry);
            }
        }
    }

    let end_session = app.provider.end_session_endpoint.as_ref()
        .ok_or_else(|| RpError::Discovery("end_session_endpoint absent".into()))?;
    let mut url = url::Url::parse(end_session)
        .map_err(|e| RpError::Discovery(e.to_string()))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("client_id", &app.cfg.client_id);
        if let Some(h) = q.id_token_hint.as_deref() {
            qp.append_pair("id_token_hint", h);
        }
        if let Some(pl) = app.cfg.post_logout_redirect_uri.as_deref() {
            qp.append_pair("post_logout_redirect_uri", pl);
        }
    }

    let mut resp = Redirect::to(url.as_str()).into_response();
    *resp.status_mut() = StatusCode::SEE_OTHER;
    let h = resp.headers_mut();
    h.append(SET_COOKIE,
        format!("{ACCESS_TOKEN_COOKIE}=; HttpOnly; Path=/; SameSite=Lax; Max-Age=0").parse().unwrap());
    h.append(SET_COOKIE,
        "expresso_rt=; HttpOnly; Path=/auth/refresh; SameSite=Lax; Max-Age=0".parse().unwrap());
    tracing::info!(target: "audit", event = "auth.logout", "user logged out");
    Ok(resp)
}

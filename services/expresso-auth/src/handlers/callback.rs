//! GET /auth/callback → token exchange (multi-tenant aware).

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{header::{ACCEPT, SET_COOKIE}, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

use expresso_auth_client::ACCESS_TOKEN_COOKIE;

use crate::error::{Result, RpError};
use crate::oidc::tokens::{AuthCodeRequest, TokenResponse};
use crate::state::AppState;
use expresso_core::audit::{self, AuditEntry};

const REFRESH_TOKEN_COOKIE: &str = "expresso_rt";

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code:  Option<String>,
    pub state: Option<String>,
    pub error:             Option<String>,
    pub error_description: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CallbackResponse {
    #[serde(flatten)]
    pub tokens: TokenResponse,
    pub post_login_redirect: Option<String>,
}

pub async fn callback(
    State(app): State<Arc<AppState>>,
    headers:    HeaderMap,
    Query(q):   Query<CallbackQuery>,
) -> Result<Response> {
    if let Some(err) = q.error {
        warn!(%err, desc = ?q.error_description, "IdP returned error");
        return Err(RpError::TokenExchange(q.error_description.unwrap_or(err)));
    }
    let code  = q.code.ok_or(RpError::BadRequest("missing code"))?;
    let state = q.state.ok_or(RpError::BadRequest("missing state"))?;

    let pending = app.take_pending(&state).await
        .ok_or(RpError::StateNotFound)?;

    // Resolve token_endpoint: per-realm when pending has realm, else static.
    let token_ep = if let Some(realm) = pending.realm.as_deref() {
        let cache = app.multi_provider.as_ref()
            .ok_or_else(|| RpError::Discovery("multi_provider missing for pending realm".into()))?;
        cache.get_or_fetch(realm).await?.token_endpoint.clone()
    } else {
        app.provider.token_endpoint.clone()
    };

    let form = AuthCodeRequest {
        grant_type:    "authorization_code",
        code:          &code,
        redirect_uri:  &pending.redirect_uri,
        client_id:     &app.cfg.client_id,
        code_verifier: &pending.code_verifier,
    };

    let resp = app.http.post(&token_ep).form(&form).send().await?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(RpError::TokenExchange(body));
    }
    let tokens: TokenResponse = resp.json().await
        .map_err(|e| RpError::TokenExchange(e.to_string()))?;

    // Validate against correct realm validator when multi-tenant.
    let ctx = if let (Some(realm), Some(mv)) = (pending.realm.as_deref(), app.multi_validator.as_ref()) {
        let v = mv.for_realm(realm).await?;
        v.validate(&tokens.access_token).await?
    } else {
        app.validator.validate(&tokens.access_token).await?
    };

    tracing::info!(
        target: "audit",
        event = "auth.login.success",
        user_id = %ctx.user_id,
        tenant_id = %ctx.tenant_id,
        email = %ctx.email,
        realm = ?pending.realm,
        "user logged in via OIDC"
    );

    if let Some(pool) = app.pool.as_ref() {
        let entry = AuditEntry {
            tenant_id:   Some(ctx.tenant_id),
            actor_sub:   Some(ctx.user_id.to_string()),
            actor_email: Some(ctx.email.clone()),
            actor_roles: ctx.roles.clone(),
            action:      "auth.login.success".into(),
            target_type: Some("user".into()),
            target_id:   Some(ctx.user_id.to_string()),
            http_method: Some("GET".into()),
            http_path:   Some("/auth/callback".into()),
            status_code: Some(200),
            metadata:    serde_json::json!({"realm": pending.realm}),
        };
        audit::record_async(pool.clone(), entry);
    }

    if let Some(fed) = crate::oidc::govbr::GovbrFederation::from_ctx(&ctx) {
        tracing::info!(
            target: "audit",
            event = "auth.federation.govbr",
            user_id = %ctx.user_id,
            tenant_id = %ctx.tenant_id,
            cpf_hash_prefix = %fed.cpf_hash_short(),
            assurance = ?fed.assurance.map(|a| a.as_str()),
            confiabilidades_count = fed.confiabilidades.len(),
            "user federated via gov.br"
        );
    }

    let json_mode = q.mode.as_deref() == Some("json")
        || headers.get(ACCEPT)
            .and_then(|h| h.to_str().ok())
            .map(|s| s.contains("application/json") && !s.contains("text/html"))
            .unwrap_or(false);

    if json_mode {
        Ok(Json(CallbackResponse {
            tokens,
            post_login_redirect: pending.post_login_redirect,
        }).into_response())
    } else {
        let secure = std::env::var("AUTH_RP__COOKIE_SECURE").ok().as_deref() == Some("1");
        let secure_attr = if secure { "; Secure" } else { "" };
        let at_cookie = format!(
            "{name}={val}; HttpOnly; Path=/; SameSite=Lax; Max-Age={max}{sec}",
            name = ACCESS_TOKEN_COOKIE,
            val  = tokens.access_token,
            max  = tokens.expires_in.max(0),
            sec  = secure_attr,
        );
        let rt_cookie = if let Some(rt) = tokens.refresh_token.as_deref() {
            let max = tokens.refresh_expires_in.unwrap_or(86_400).max(0);
            Some(format!(
                "{name}={val}; HttpOnly; Path=/auth/refresh; SameSite=Lax; Max-Age={max}{sec}",
                name = REFRESH_TOKEN_COOKIE,
                val  = rt,
                max  = max,
                sec  = secure_attr,
            ))
        } else { None };

        let target = pending.post_login_redirect.unwrap_or_else(|| "/".to_string());
        let mut resp = Redirect::to(&target).into_response();
        resp.headers_mut().append(SET_COOKIE, at_cookie.parse().unwrap());
        if let Some(rt) = rt_cookie {
            resp.headers_mut().append(SET_COOKIE, rt.parse().unwrap());
        }
        *resp.status_mut() = StatusCode::SEE_OTHER;
        Ok(resp)
    }
}

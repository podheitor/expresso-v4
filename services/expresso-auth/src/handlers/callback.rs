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
        // Mesma defesa do ramo HTML: nunca devolve um redirect arbitrário,
        // pois o cliente SPA tipicamente faz `window.location = …` sem
        // checar de novo. Mantém o campo Some/None pra preservar o shape.
        let safe = pending
            .post_login_redirect
            .as_deref()
            .filter(|s| is_safe_local_redirect(s))
            .map(str::to_string);
        Ok(Json(CallbackResponse {
            tokens,
            post_login_redirect: safe,
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

        // post_login_redirect vem de `?redirect_uri=...` no /auth/login —
        // entrada do usuário, e o atacante pode usar /auth/login?redirect_uri=
        // https://evil.com pra phishing pós-login (vítima logou de verdade,
        // a URL final parece confiável). Só aceitamos caminhos relativos
        // mesma-origem; qualquer outra coisa cai pra `/`.
        let target = pending
            .post_login_redirect
            .as_deref()
            .filter(|s| is_safe_local_redirect(s))
            .unwrap_or("/")
            .to_string();
        let mut resp = Redirect::to(&target).into_response();
        resp.headers_mut().append(SET_COOKIE, at_cookie.parse().unwrap());
        if let Some(rt) = rt_cookie {
            resp.headers_mut().append(SET_COOKIE, rt.parse().unwrap());
        }
        *resp.status_mut() = StatusCode::SEE_OTHER;
        Ok(resp)
    }
}

/// True quando `s` é um caminho relativo seguro (mesma origem) — i.e.
/// começa com `/`, não é protocol-relative (`//host`), não usa `\`
/// (algumas implementações normalizam pra `/` e podem virar `\\host`),
/// e não tem CR/LF (header injection).
fn is_safe_local_redirect(s: &str) -> bool {
    if !s.starts_with('/') { return false; }
    // Protocol-relative: `//evil.com/path` — Redirect::to em axum
    // deixaria o browser pular pra outro host.
    if s.starts_with("//") || s.starts_with("/\\") { return false; }
    // Backslash → alguns browsers (IE/edge legados) tratam como `/`,
    // então `/\\evil.com` viraria `//evil.com`. Bloqueia tudo com `\`.
    if s.contains('\\') { return false; }
    // Newlines em Location: header → injeção.
    if s.contains('\r') || s.contains('\n') { return false; }
    true
}

#[cfg(test)]
mod tests {
    use super::is_safe_local_redirect;

    #[test]
    fn accepts_relative_paths() {
        assert!(is_safe_local_redirect("/"));
        assert!(is_safe_local_redirect("/inbox"));
        assert!(is_safe_local_redirect("/inbox?folder=42"));
        assert!(is_safe_local_redirect("/path/to/page#anchor"));
    }

    #[test]
    fn rejects_absolute_urls() {
        assert!(!is_safe_local_redirect("https://evil.com/x"));
        assert!(!is_safe_local_redirect("http://evil.com"));
        assert!(!is_safe_local_redirect("javascript:alert(1)"));
        assert!(!is_safe_local_redirect("data:text/html,<x>"));
    }

    #[test]
    fn rejects_protocol_relative_and_backslash_tricks() {
        assert!(!is_safe_local_redirect("//evil.com/x"));
        assert!(!is_safe_local_redirect("/\\evil.com"));
        assert!(!is_safe_local_redirect("/path\\with-bs"));
    }

    #[test]
    fn rejects_crlf_injection() {
        assert!(!is_safe_local_redirect("/path\r\nLocation: https://evil"));
        assert!(!is_safe_local_redirect("/path\nfoo"));
    }

    #[test]
    fn rejects_empty_and_relative_no_slash() {
        assert!(!is_safe_local_redirect(""));
        assert!(!is_safe_local_redirect("inbox"));
        assert!(!is_safe_local_redirect("./inbox"));
    }
}

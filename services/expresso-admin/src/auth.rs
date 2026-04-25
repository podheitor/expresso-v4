//! OIDC group/role gate. Forwards browser cookie to expresso-auth `/auth/me`,
//! requires the resulting principal to have at least one of `ADMIN_ROLES`.
//! Bypasses static / health / metrics paths.

use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub auth_base:    String,
    pub admin_roles:  Vec<String>,
    pub login_path:   String,
    /// Iff true, admins must have performed TOTP/WebAuthn step-up (via `mfa.totp`|`mfa.webauthn`)
    /// for the current session. Controlled via `ADMIN_REQUIRE_2FA` env (default false).
    pub require_2fa:  bool,
}

impl AuthConfig {
    pub fn from_env() -> Self {
        Self {
            auth_base: std::env::var("BACKEND__AUTH").unwrap_or_else(|_| "http://expresso-auth:8012".into()),
            admin_roles: std::env::var("ADMIN_ROLES")
                .unwrap_or_else(|_| "super_admin,tenant_admin".into())
                .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
            login_path: std::env::var("PUBLIC__AUTH_LOGIN").unwrap_or_else(|_| "/auth/login".into()),
            require_2fa: std::env::var("ADMIN_REQUIRE_2FA")
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MfaField {
    #[serde(default)]
    pub totp:     bool,
    #[serde(default)]
    pub webauthn: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MeResp {
    #[serde(default)]
    pub roles:     Vec<String>,
    #[serde(default)]
    pub user_id:   Option<uuid::Uuid>,
    #[serde(default)]
    pub tenant_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub email:     Option<String>,
    #[serde(default)]
    pub mfa:       MfaField,
}

fn is_public_path(p: &str) -> bool {
    p == "/health" || p == "/ready" || p.starts_with("/static") || p.starts_with("/metrics") || p == "/forbidden"
}

fn login_redirect(login_path: &str, uri: &Uri) -> Response {
    let target = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let enc = utf8_percent_encode(target, NON_ALPHANUMERIC).to_string();
    Redirect::to(&format!("{login_path}?redirect={enc}")).into_response()
}

pub async fn require_admin(
    State(st): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let uri = req.uri().clone();
    if is_public_path(uri.path()) {
        return next.run(req).await;
    }
    let cookie = req.headers().get(header::COOKIE).cloned();

    // No cookie → straight to login.
    let Some(cookie_v) = cookie else {
        return login_redirect(&st.auth.login_path, &uri);
    };

    let me_url = format!("{}/auth/me", st.auth.auth_base.trim_end_matches('/'));
    let resp = match st.http.get(&me_url).header(header::COOKIE, cookie_v).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "auth backend unreachable");
            return (StatusCode::BAD_GATEWAY, "auth backend unreachable").into_response();
        }
    };
    if resp.status() == StatusCode::UNAUTHORIZED {
        return login_redirect(&st.auth.login_path, &uri);
    }
    if !resp.status().is_success() {
        return (StatusCode::BAD_GATEWAY, format!("auth/me {}", resp.status())).into_response();
    }
    let me: MeResp = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "auth/me parse");
            return (StatusCode::BAD_GATEWAY, "auth/me parse").into_response();
        }
    };

    let allowed = me.roles.iter().any(|r| st.auth.admin_roles.iter().any(|a| a.eq_ignore_ascii_case(r) || a.replace('_', "").eq_ignore_ascii_case(&r.replace('_', ""))));
    if !allowed {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            format!(
                "<!doctype html><meta charset=utf-8><title>403</title>\
                 <body style=\"font-family:system-ui;padding:2rem\">\
                 <h1>403 — Acesso negado</h1>\
                 <p>Sua conta não possui permissão para acessar o painel administrativo.</p>\
                 <p class=muted>Roles requeridas: <code>{}</code></p>\
                 <p>Roles atuais: <code>{}</code></p>\
                 </body>",
                st.auth.admin_roles.join(", "),
                me.roles.join(", ")
            ),
        ).into_response();
    }

    // 2FA gate — require TOTP or WebAuthn step-up when enabled.
    if st.auth.require_2fa && !(me.mfa.totp || me.mfa.webauthn) {
        tracing::warn!(
            user = ?me.user_id,
            email = ?me.email,
            "admin access denied: 2FA required but not present"
        );
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "<!doctype html><meta charset=utf-8><title>2FA obrigatória</title>\
             <body style=\"font-family:system-ui;padding:2rem;max-width:40rem;margin:auto\">\
             <h1>Autenticação em 2 fatores obrigatória</h1>\
             <p>O painel administrativo exige autenticação em 2 fatores (TOTP ou chave de segurança).</p>\
             <p>Sua sessão atual <strong>não</strong> foi elevada com 2FA.</p>\
             <ol>\
               <li>Faça logout e entre novamente informando o código TOTP, ou</li>\
               <li>Registre TOTP no seu perfil Keycloak antes de tentar de novo.</li>\
             </ol>\
             <p><a href=\"/auth/logout\">Sair e tentar de novo</a></p>\
             </body>"
        ).into_response();
    }

    next.run(req).await
}

// ─── Super-admin gate for tenant management ─────────────────────────────────

/// Fetch `/auth/me` roles list for the request. Returns empty vec on failure.
pub async fn roles_for(st: &AppState, headers: &axum::http::HeaderMap) -> Vec<String> {
    let Some(cookie) = headers.get(header::COOKIE).cloned() else { return vec![]; };
    let me_url = format!("{}/auth/me", st.auth.auth_base.trim_end_matches('/'));
    let Ok(resp) = st.http.get(&me_url).header(header::COOKIE, cookie).send().await else {
        return vec![];
    };
    if !resp.status().is_success() { return vec![]; }
    resp.json::<MeResp>().await.map(|m| m.roles).unwrap_or_default()
}

/// Returns `None` if caller holds `super_admin`; otherwise returns a 403 response.
pub async fn require_super_admin(st: &AppState, headers: &axum::http::HeaderMap) -> Option<Response> {
    let roles = roles_for(st, headers).await;
    if roles.iter().any(|r| r.eq_ignore_ascii_case("super_admin") || r.eq_ignore_ascii_case("superadmin")) {
        None
    } else {
        Some((
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            format!(
                "<!doctype html><meta charset=utf-8><title>403</title>\
                 <body style=\"font-family:system-ui;padding:2rem\">\
                 <h1>403 — Requer super_admin</h1>\
                 <p>Gestão de tenants exige role <code>super_admin</code>.</p>\
                 <p>Roles atuais: <code>{}</code></p>\
                 </body>",
                roles.join(", ")
            ),
        ).into_response())
    }
}

/// True when the caller has `super_admin` (case-insensitive, with or without
/// underscore). Super-admins may operate across tenants; everyone else is
/// confined to their own tenant_id.
pub fn is_super_admin(roles: &[String]) -> bool {
    roles.iter().any(|r| {
        r.eq_ignore_ascii_case("super_admin") || r.eq_ignore_ascii_case("superadmin")
    })
}

/// Validates a tenant-scoped admin operation: super-admins pass through,
/// everyone else must operate on their *own* tenant_id (URL path must match
/// the principal's tenant). Returns a 403 response on mismatch.
///
/// Called at the top of every per-tenant DAV-admin handler — without it, a
/// tenant_admin can supply an arbitrary tenant_id in the URL path and the
/// underlying SQL (which only filters by the path-supplied id) will operate
/// on another tenant's rows. Defense-in-depth: prefer this over relying on
/// audit-log forensics to spot the abuse after the fact.
pub async fn require_tenant_match(
    st:        &AppState,
    headers:   &axum::http::HeaderMap,
    requested: uuid::Uuid,
) -> Option<Response> {
    let p = principal_for(st, headers).await;
    if is_super_admin(&p.roles) {
        return None;
    }
    match p.tenant_id {
        Some(t) if t == requested => None,
        _ => {
            tracing::warn!(
                user = ?p.user_id,
                principal_tenant = ?p.tenant_id,
                requested_tenant = %requested,
                "cross-tenant admin op blocked"
            );
            Some((
                StatusCode::FORBIDDEN,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                "<!doctype html><meta charset=utf-8><title>403</title>\
                 <body style=\"font-family:system-ui;padding:2rem\">\
                 <h1>403 — Operação cross-tenant negada</h1>\
                 <p>Apenas <code>super_admin</code> pode operar fora do próprio tenant.</p>\
                 </body>",
            ).into_response())
        }
    }
}

/// Fetch the full principal (sub + email + roles) via `/auth/me`. Empty struct on failure.
pub async fn principal_for(st: &AppState, headers: &axum::http::HeaderMap) -> MeResp {
    let Some(cookie) = headers.get(axum::http::header::COOKIE).cloned() else {
        return MeResp::default();
    };
    let me_url = format!("{}/auth/me", st.auth.auth_base.trim_end_matches('/'));
    let Ok(resp) = st.http.get(&me_url).header(axum::http::header::COOKIE, cookie).send().await else {
        return MeResp::default();
    };
    if !resp.status().is_success() {
        return MeResp::default();
    }
    resp.json::<MeResp>().await.unwrap_or(MeResp::default())
}

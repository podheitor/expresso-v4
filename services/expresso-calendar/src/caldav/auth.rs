//! CalDAV HTTP Basic authentication.
//!
//! Order of precedence per request:
//! 1. If `AppState::kc_basic()` is configured → delegate to Keycloak password
//!    grant. On success, resolve the DB user row by email.
//! 2. Else if `CALENDAR_DEV_AUTH=1` → dev mode: resolve user by email, accept
//!    any password. Never enabled in production.
//! 3. Else → 401.
//!
//! Tenant scoping: `users.email` is per-tenant unique (`UNIQUE(tenant_id, email)`),
//! NOT globally. Resolving by email alone could pick an arbitrary tenant's row
//! if two tenants share an email. We resolve the tenant from the `Host` header
//! via `TenantResolver` (realm name = tenant UUID per the realm-per-tenant
//! convention) and filter the lookup by that tenant. If no resolver is wired
//! or the host is unmapped, we detect ambiguity (>1 row) and reject.

use std::sync::Arc;

use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{header, request::Parts, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use expresso_auth_client::{KcBasicError, TenantResolver};
use uuid::Uuid;

use crate::state::AppState;

#[derive(Debug, Clone, Copy)]
pub struct CalDavPrincipal {
    pub tenant_id: Uuid,
    pub user_id:   Uuid,
}

#[async_trait]
impl FromRequestParts<AppState> for CalDavPrincipal {
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AuthError> {
        let header_val = parts
            .headers
            .get(header::AUTHORIZATION)
            .ok_or(AuthError::Missing)?
            .to_str()
            .map_err(|_| AuthError::Malformed)?;

        let (user, pass) = decode_basic(header_val).ok_or(AuthError::Malformed)?;

        let tenant_hint = resolve_tenant_from_host(parts);

        // 1) Keycloak path (production).
        if let Some(kc) = state.kc_basic() {
            match kc.authenticate(&user, &pass).await {
                Ok(_)                                     => return resolve_user(state, &user, tenant_hint).await,
                Err(KcBasicError::InvalidCredentials)     => return Err(AuthError::Forbidden),
                Err(KcBasicError::Unreachable(_))         => return Err(AuthError::Unavailable),
                Err(KcBasicError::Upstream(_))            => return Err(AuthError::Unavailable),
            }
        }

        // 2) Dev path.
        if std::env::var("CALENDAR_DEV_AUTH").ok().as_deref() == Some("1") {
            return resolve_user(state, &user, tenant_hint).await;
        }

        // 3) No auth backend configured.
        Err(AuthError::Forbidden)
    }
}

/// Resolve `Host` header → realm via `TenantResolver` → tenant UUID.
/// Returns `None` when resolver is not wired, host header is missing/invalid,
/// host is not mapped, or the realm name does not parse as a UUID.
fn resolve_tenant_from_host(parts: &Parts) -> Option<Uuid> {
    let resolver = parts.extensions.get::<Arc<TenantResolver>>()?;
    let host = parts.headers.get(header::HOST).and_then(|v| v.to_str().ok())?;
    let realm = resolver.resolve(host)?;
    Uuid::parse_str(realm.trim()).ok()
}

async fn resolve_user(
    state:       &AppState,
    user:        &str,
    tenant_hint: Option<Uuid>,
) -> Result<CalDavPrincipal, AuthError> {
    let pool = state.db().ok_or(AuthError::Unavailable)?;

    if let Some(tenant_id) = tenant_hint {
        let row: Option<(Uuid,)> = sqlx::query_as(
            r#"SELECT id FROM users WHERE tenant_id = $1 AND email = $2 LIMIT 1"#,
        )
        .bind(tenant_id)
        .bind(user)
        .fetch_optional(pool)
        .await
        .map_err(|_| AuthError::Unavailable)?;
        let (user_id,) = row.ok_or(AuthError::Forbidden)?;
        return Ok(CalDavPrincipal { tenant_id, user_id });
    }

    // No host→tenant mapping: detect ambiguity. If exactly one tenant owns
    // this email, accept it; otherwise refuse rather than guess.
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        r#"SELECT tenant_id, id FROM users WHERE email = $1 LIMIT 2"#,
    )
    .bind(user)
    .fetch_all(pool)
    .await
    .map_err(|_| AuthError::Unavailable)?;
    match rows.as_slice() {
        [(tenant_id, user_id)] => Ok(CalDavPrincipal { tenant_id: *tenant_id, user_id: *user_id }),
        []                     => Err(AuthError::Forbidden),
        _                      => {
            tracing::warn!(email = %user, "ambiguous CalDAV login: email exists in multiple tenants and no Host→tenant mapping wired");
            Err(AuthError::Forbidden)
        }
    }
}

/// Decode `Basic <b64(user:pass)>` → (user, pass).
fn decode_basic(header: &str) -> Option<(String, String)> {
    let token = header.strip_prefix("Basic ").or_else(|| header.strip_prefix("basic "))?;
    let decoded = STANDARD.decode(token.trim()).ok()?;
    let s = String::from_utf8(decoded).ok()?;
    let (u, p) = s.split_once(':')?;
    Some((u.to_owned(), p.to_owned()))
}

#[derive(Debug)]
pub enum AuthError {
    Missing,
    Malformed,
    Forbidden,
    Unavailable,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let challenge = HeaderValue::from_static("Basic realm=\"Expresso CalDAV\", charset=\"UTF-8\"");
        let (status, msg) = match self {
            Self::Missing     => (StatusCode::UNAUTHORIZED,        "authentication required"),
            Self::Malformed   => (StatusCode::BAD_REQUEST,         "malformed Authorization header"),
            Self::Forbidden   => (StatusCode::UNAUTHORIZED,        "invalid credentials"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "auth backend unavailable"),
        };
        let mut resp = (status, msg).into_response();
        if matches!(status, StatusCode::UNAUTHORIZED) {
            resp.headers_mut().insert(header::WWW_AUTHENTICATE, challenge);
        }
        resp
    }
}

//! Request context extractor — dual-mode.
//!
//! Mode 1 (strict, production): if `Arc<OidcValidator>` is present in the
//! request extensions, parse the `Authorization: Bearer …` header, validate
//! the JWT (signature + iss + aud + exp) and derive tenant/user from claims.
//!
//! Mode 2 (header fallback, dev): if no validator is wired, read
//! `X-Tenant-Id` + `X-User-Id` from headers. Guarded by service startup logs
//! so operators notice when they're running insecure.

use std::sync::Arc;

use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{
        header::{HeaderMap, AUTHORIZATION, COOKIE, HOST},
        request::Parts,
        StatusCode,
    },
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use uuid::Uuid;

use expresso_auth_client::metrics as auth_metrics;
use expresso_auth_client::{
    AuthError, MultiRealmValidator, OidcValidator, TenantResolver, ACCESS_TOKEN_COOKIE,
};

pub const H_TENANT: &str = "x-tenant-id";
pub const H_USER: &str = "x-user-id";
pub const H_ROLES: &str = "x-roles";

#[derive(Debug, Clone, Copy)]
pub struct RequestCtx {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
}

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for RequestCtx {
    type Rejection = CtxError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let ResolvedCtx {
            tenant_id, user_id, ..
        } = resolve_ctx(parts).await?;
        Ok(Self { tenant_id, user_id })
    }
}

/// Resource-registry admin gate. Same authentication as [`RequestCtx`], but the
/// caller must additionally hold a tenant-admin role — registering or deleting
/// bookable rooms is an admin operation, not a per-user one. Mirrors
/// expresso-auth's `is_super` check, scoped to the calendar admin roles.
pub struct AdminCtx {
    pub tenant_id: Uuid,
}

/// Roles accepted as tenant calendar admins (case-insensitive).
const ADMIN_ROLES: [&str; 3] = ["admin", "calendar-admin", "calendar_admin"];

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for AdminCtx {
    type Rejection = CtxError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let c = resolve_ctx(parts).await?;
        if !has_admin_role(&c.roles) {
            return Err(CtxError::Forbidden("admin_role_required".into()));
        }
        Ok(Self {
            tenant_id: c.tenant_id,
        })
    }
}

/// True if any role matches an entry in [`ADMIN_ROLES`] (case-insensitive).
fn has_admin_role(roles: &[String]) -> bool {
    roles
        .iter()
        .any(|r| ADMIN_ROLES.iter().any(|a| r.eq_ignore_ascii_case(a)))
}

/// Authenticated identity plus roles, resolved once and shared by both
/// extractors. In header-fallback (dev) mode, roles come from `X-Roles`.
struct ResolvedCtx {
    tenant_id: Uuid,
    user_id: Uuid,
    roles: Vec<String>,
}

/// Validate the request and produce a [`ResolvedCtx`], using the same dual-mode
/// precedence as the original `RequestCtx` extractor: multi-realm → single-realm
/// OIDC → dev header fallback.
async fn resolve_ctx(parts: &mut Parts) -> Result<ResolvedCtx, CtxError> {
    // JWT mode: validator wired → Bearer required, strict validation.
    // Multi-realm precedence: if MultiRealmValidator + TenantResolver
    // present → resolve realm via Host header and validate.
    if let (Some(m), Some(r)) = (
        parts.extensions.get::<Arc<MultiRealmValidator>>().cloned(),
        parts.extensions.get::<Arc<TenantResolver>>().cloned(),
    ) {
        if let Some(host) = parts.headers.get(HOST).and_then(|v| v.to_str().ok()) {
            if let Some(realm) = r.resolve(host) {
                let token = require_token(&parts.headers)?;
                let v = m.for_realm(realm).await.map_err(|e| {
                    auth_metrics::VALIDATION_TOTAL
                        .with_label_values(&[realm, auth_metrics::result_label(&e)])
                        .inc();
                    CtxError::from(e)
                })?;
                return match v.validate(&token).await {
                    Ok(c) => {
                        auth_metrics::VALIDATION_TOTAL
                            .with_label_values(&[realm, "ok"])
                            .inc();
                        Ok(ResolvedCtx {
                            tenant_id: c.tenant_id,
                            user_id: c.user_id,
                            roles: c.roles,
                        })
                    }
                    Err(e) => {
                        auth_metrics::VALIDATION_TOTAL
                            .with_label_values(&[realm, auth_metrics::result_label(&e)])
                            .inc();
                        Err(CtxError::from(e))
                    }
                };
            }
        }
    }

    if let Some(validator) = parts.extensions.get::<Arc<OidcValidator>>().cloned() {
        let token = require_token(&parts.headers)?;
        let ctx = validator.validate(&token).await.map_err(CtxError::from)?;
        return Ok(ResolvedCtx {
            tenant_id: ctx.tenant_id,
            user_id: ctx.user_id,
            roles: ctx.roles,
        });
    }

    // Header fallback (dev only).
    let tenant_id =
        parse_uuid_header(&parts.headers, H_TENANT).ok_or(CtxError::MissingHeader(H_TENANT))?;
    let user_id =
        parse_uuid_header(&parts.headers, H_USER).ok_or(CtxError::MissingHeader(H_USER))?;
    Ok(ResolvedCtx {
        tenant_id,
        user_id,
        roles: parse_roles_header(&parts.headers),
    })
}

/// Extract the bearer or access-token-cookie value, or fail with `MissingBearer`.
/// Returns an owned `String` so both header and cookie sources unify (the cookie
/// value is not a sub-slice of a single header value).
fn require_token(h: &HeaderMap) -> Result<String, CtxError> {
    if let Some(t) = bearer_token(h) {
        return Ok(t.to_string());
    }
    cookie_token(h, ACCESS_TOKEN_COOKIE).ok_or(CtxError::MissingBearer)
}

/// Parse the dev-only `X-Roles` header (comma-separated) into a role list.
/// Empty/absent → no roles. Only consulted in header-fallback mode.
fn parse_roles_header(h: &HeaderMap) -> Vec<String> {
    h.get(H_ROLES)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_uuid_header(h: &HeaderMap, name: &'static str) -> Option<Uuid> {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s.trim()).ok())
}

fn bearer_token(h: &HeaderMap) -> Option<&str> {
    let raw = h.get(AUTHORIZATION)?.to_str().ok()?;
    let rest = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))?;
    let t = rest.trim();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn cookie_token(h: &HeaderMap, name: &str) -> Option<String> {
    for hv in h.get_all(COOKIE).iter() {
        let s = match hv.to_str() {
            Ok(v) => v,
            Err(_) => continue,
        };
        for pair in s.split(';') {
            let pair = pair.trim();
            if let Some((k, v)) = pair.split_once('=') {
                if k.trim() == name {
                    let v = v.trim();
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    None
}

#[derive(Debug)]
pub enum CtxError {
    MissingHeader(&'static str),
    MissingBearer,
    InvalidToken(String),
    Expired,
    Forbidden(String),
}

impl From<AuthError> for CtxError {
    fn from(e: AuthError) -> Self {
        match e {
            AuthError::Expired => Self::Expired,
            AuthError::MissingBearer => Self::MissingBearer,
            AuthError::InvalidToken(m) => Self::InvalidToken(m),
            AuthError::KidNotFound(_) => Self::InvalidToken("unknown_key".into()),
            AuthError::MalformedClaim(n, m) => Self::InvalidToken(format!("malformed_{n}: {m}")),
            AuthError::MissingClaim(n) => Self::Forbidden(format!("missing_{n}")),
            AuthError::Config(m) | AuthError::JwksFetch(m) => Self::InvalidToken(m),
        }
    }
}

impl IntoResponse for CtxError {
    fn into_response(self) -> Response {
        let (status, code, msg) = match self {
            Self::MissingHeader(h) => (StatusCode::UNAUTHORIZED, "missing_header", h.to_string()),
            Self::MissingBearer => (
                StatusCode::UNAUTHORIZED,
                "missing_bearer",
                "Authorization: Bearer <jwt> required".into(),
            ),
            Self::InvalidToken(m) => (StatusCode::UNAUTHORIZED, "invalid_token", m),
            Self::Expired => (StatusCode::UNAUTHORIZED, "token_expired", "expired".into()),
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, "forbidden", m),
        };
        (status, Json(json!({"error": code, "message": msg}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(rs: &[&str]) -> Vec<String> {
        rs.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn admin_role_accepts_plain_admin() {
        assert!(has_admin_role(&roles(&["user", "admin"])));
    }

    #[test]
    fn admin_role_is_case_insensitive() {
        assert!(has_admin_role(&roles(&["Calendar-Admin"])));
        assert!(has_admin_role(&roles(&["ADMIN"])));
    }

    #[test]
    fn admin_role_accepts_underscore_and_dash_variants() {
        assert!(has_admin_role(&roles(&["calendar_admin"])));
        assert!(has_admin_role(&roles(&["calendar-admin"])));
    }

    #[test]
    fn admin_role_rejects_non_admin() {
        assert!(!has_admin_role(&roles(&["user", "editor"])));
        assert!(!has_admin_role(&[]));
        // A role merely containing "admin" as a substring is not a match.
        assert!(!has_admin_role(&roles(&["administrator", "adminx"])));
    }

    #[test]
    fn roles_header_parses_csv_and_trims() {
        let mut h = HeaderMap::new();
        h.insert(H_ROLES, "admin, user ,editor".parse().unwrap());
        assert_eq!(parse_roles_header(&h), roles(&["admin", "user", "editor"]));
    }

    #[test]
    fn roles_header_absent_is_empty() {
        assert!(parse_roles_header(&HeaderMap::new()).is_empty());
    }

    #[test]
    fn roles_header_blank_entries_dropped() {
        let mut h = HeaderMap::new();
        h.insert(H_ROLES, " , admin , ".parse().unwrap());
        assert_eq!(parse_roles_header(&h), roles(&["admin"]));
    }
}

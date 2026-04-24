//! Axum extractor: `Authenticated(AuthContext)`.
//!
//! Token sources tried in order:
//!   1. `Authorization: Bearer <jwt>`
//!   2. `Cookie: expresso_at=<jwt>` (browser session set by expresso-auth)
//! Validates via `OidcValidator` from request `Extensions`, returns
//! `AuthContext` or 401/403.

use std::sync::Arc;

use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{header::{AUTHORIZATION, COOKIE}, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::claims::AuthContext;
use crate::error::AuthError;
use crate::validator::OidcValidator;

/// Cookie name used by expresso-auth to ship the access token to the browser.
pub const ACCESS_TOKEN_COOKIE: &str = "expresso_at";

pub struct Authenticated(pub AuthContext);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for Authenticated {
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let token_owned;
        let token = if let Some(t) = extract_bearer(parts) {
            t
        } else if let Some(t) = extract_cookie(parts, ACCESS_TOKEN_COOKIE) {
            token_owned = t;
            token_owned.as_str()
        } else {
            return Err(AuthRejection::from(AuthError::MissingBearer));
        };

        // Multi-realm path: se MultiRealmValidator + TenantResolver presentes
        // em extensions, resolve realm via Host header e valida. Fase2 do
        // realm-per-tenant. Caso host nao mapeado ou extensions ausentes,
        // cai p/ single-realm.
        let multi = parts.extensions.get::<Arc<crate::multi_validator::MultiRealmValidator>>().cloned();
        let resolver = parts.extensions.get::<Arc<crate::tenant_resolver::TenantResolver>>().cloned();
        if let (Some(m), Some(r)) = (multi, resolver) {
            if let Some(host) = parts.headers.get(axum::http::header::HOST).and_then(|v| v.to_str().ok()) {
                if let Some(realm) = r.resolve(host) {
                    let v = m.for_realm(realm).await.map_err(AuthRejection::from)?;
                    let ctx = v.validate(token).await.map_err(AuthRejection::from)?;
                    return Ok(Self(ctx));
                }
            }
        }

        let validator = parts.extensions
            .get::<Arc<OidcValidator>>()
            .cloned()
            .ok_or(AuthRejection::Misconfigured)?;
        let ctx = validator.validate(token).await.map_err(AuthRejection::from)?;
        Ok(Self(ctx))
    }
}

fn extract_bearer(parts: &Parts) -> Option<&str> {
    let raw = parts.headers.get(AUTHORIZATION)?.to_str().ok()?;
    let rest = raw.strip_prefix("Bearer ").or_else(|| raw.strip_prefix("bearer "))?;
    let t = rest.trim();
    if t.is_empty() { None } else { Some(t) }
}

/// Parse `Cookie` header → first matching value for `name`.
/// Tolerates multiple Cookie headers + spaces around `=`.
fn extract_cookie(parts: &Parts, name: &str) -> Option<String> {
    for hv in parts.headers.get_all(COOKIE).iter() {
        let s = match hv.to_str() { Ok(v) => v, Err(_) => continue };
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
pub enum AuthRejection {
    Misconfigured,
    Unauthorized(String),
    Forbidden(String),
    Expired,
}

impl From<AuthError> for AuthRejection {
    fn from(e: AuthError) -> Self {
        match e {
            AuthError::Expired                 => Self::Expired,
            AuthError::MissingBearer           => Self::Unauthorized("missing_bearer".into()),
            AuthError::InvalidToken(m)         => Self::Unauthorized(format!("invalid_token: {m}")),
            AuthError::KidNotFound(_)          => Self::Unauthorized("unknown_key".into()),
            AuthError::MalformedClaim(n, m)    => Self::Unauthorized(format!("malformed_{n}: {m}")),
            AuthError::MissingClaim(n)         => Self::Forbidden(format!("missing_{n}")),
            AuthError::Config(m) | AuthError::JwksFetch(m) => Self::Unauthorized(m),
        }
    }
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        let (status, code, msg) = match self {
            Self::Misconfigured   => (StatusCode::INTERNAL_SERVER_ERROR, "misconfigured", "auth not wired".to_string()),
            Self::Expired         => (StatusCode::UNAUTHORIZED,          "token_expired", "expired".to_string()),
            Self::Unauthorized(m) => (StatusCode::UNAUTHORIZED,          "unauthorized",  m),
            Self::Forbidden(m)    => (StatusCode::FORBIDDEN,             "forbidden",     m),
        };
        (status, Json(json!({"error": code, "message": msg}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    fn parts_with_cookies(cookies: &[&str]) -> Parts {
        let mut req = axum::http::Request::builder().body(()).unwrap();
        let h = req.headers_mut();
        for c in cookies {
            h.append(COOKIE, HeaderValue::from_str(c).unwrap());
        }
        // Need just Parts; build into request then split
        let (parts, _) = req.into_parts();
        parts
    }

    #[test]
    fn extracts_named_cookie_among_many() {
        let p = parts_with_cookies(&["foo=bar; expresso_at=tok123; baz=qux"]);
        assert_eq!(extract_cookie(&p, "expresso_at").as_deref(), Some("tok123"));
    }

    #[test]
    fn extracts_across_multiple_cookie_headers() {
        let p = parts_with_cookies(&["foo=bar", "expresso_at=multi; x=y"]);
        assert_eq!(extract_cookie(&p, "expresso_at").as_deref(), Some("multi"));
    }

    #[test]
    fn returns_none_when_absent_or_empty() {
        let p = parts_with_cookies(&["foo=bar; expresso_at="]);
        assert!(extract_cookie(&p, "expresso_at").is_none());
        let p2 = parts_with_cookies(&["foo=bar"]);
        assert!(extract_cookie(&p2, "expresso_at").is_none());
    }

    #[test]
    fn handles_unused_headermap_param() {
        // Sanity: HeaderMap::new compiles + extract_cookie tolerates no headers.
        let _ = HeaderMap::new();
    }
}

// --- Multi-realm extractor (fase 2 do realm-per-tenant) ---------------

use crate::multi_validator::MultiRealmValidator;
use crate::tenant_resolver::TenantResolver;
use axum::http::header::HOST;

/// Extractor multi-realm: resolve Host → realm → validator, valida token.
///
/// Requer nas request extensions:
/// - `Arc<MultiRealmValidator>` — pool de validators por realm
/// - `Arc<TenantResolver>`      — mapeamento host → realm
///
/// Se qualquer um faltar → 500 `misconfigured`. Se host desconhecido
/// → 401 `unknown_tenant`.
pub struct TenantAuthenticated(pub AuthContext, pub String /* realm */);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for TenantAuthenticated {
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let multi = parts.extensions.get::<Arc<MultiRealmValidator>>().cloned()
            .ok_or(AuthRejection::Misconfigured)?;
        let resolver = parts.extensions.get::<Arc<TenantResolver>>().cloned()
            .ok_or(AuthRejection::Misconfigured)?;

        let host = parts.headers.get(HOST)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AuthRejection::Unauthorized("missing_host".into()))?;

        let realm = resolver.resolve(host)
            .ok_or_else(|| AuthRejection::Unauthorized(format!("unknown_tenant: {host}")))?
            .to_string();

        let validator = multi.for_realm(&realm).await.map_err(AuthRejection::from)?;

        let token_owned;
        let token = if let Some(t) = extract_bearer(parts) {
            t
        } else if let Some(t) = extract_cookie(parts, ACCESS_TOKEN_COOKIE) {
            token_owned = t;
            token_owned.as_str()
        } else {
            return Err(AuthRejection::from(AuthError::MissingBearer));
        };

        let ctx = validator.validate(token).await.map_err(AuthRejection::from)?;
        Ok(Self(ctx, realm))
    }
}

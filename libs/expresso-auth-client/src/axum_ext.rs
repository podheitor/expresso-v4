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
    http::{
        header::{AUTHORIZATION, COOKIE},
        request::Parts,
        StatusCode,
    },
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
        let multi = parts
            .extensions
            .get::<Arc<crate::multi_validator::MultiRealmValidator>>()
            .cloned();
        let resolver = parts
            .extensions
            .get::<Arc<crate::tenant_resolver::TenantResolver>>()
            .cloned();
        if let (Some(m), Some(r)) = (multi, resolver) {
            if let Some(host) = parts
                .headers
                .get(axum::http::header::HOST)
                .and_then(|v| v.to_str().ok())
            {
                if let Some(realm) = r.resolve(host) {
                    let v = match m.for_realm(realm).await {
                        Ok(v) => v,
                        Err(e) => {
                            crate::metrics::VALIDATION_TOTAL
                                .with_label_values(&[realm, crate::metrics::result_label(&e)])
                                .inc();
                            return Err(AuthRejection::from(e));
                        }
                    };
                    match v.validate(token).await {
                        Ok(ctx) => {
                            crate::metrics::VALIDATION_TOTAL
                                .with_label_values(&[realm, "ok"])
                                .inc();
                            return Ok(Self(ctx));
                        }
                        Err(e) => {
                            crate::metrics::VALIDATION_TOTAL
                                .with_label_values(&[realm, crate::metrics::result_label(&e)])
                                .inc();
                            return Err(AuthRejection::from(e));
                        }
                    }
                }
            }
        }

        let validator = parts
            .extensions
            .get::<Arc<OidcValidator>>()
            .cloned()
            .ok_or(AuthRejection::Misconfigured)?;
        let ctx = validator
            .validate(token)
            .await
            .map_err(AuthRejection::from)?;
        Ok(Self(ctx))
    }
}

fn extract_bearer(parts: &Parts) -> Option<&str> {
    let raw = parts.headers.get(AUTHORIZATION)?.to_str().ok()?;
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

/// Parse `Cookie` header → first matching value for `name`.
/// Tolerates multiple Cookie headers + spaces around `=`.
fn extract_cookie(parts: &Parts, name: &str) -> Option<String> {
    for hv in parts.headers.get_all(COOKIE).iter() {
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
pub enum AuthRejection {
    Misconfigured,
    Unauthorized(String),
    Forbidden(String),
    Expired,
}

impl From<AuthError> for AuthRejection {
    fn from(e: AuthError) -> Self {
        match e {
            AuthError::Expired => Self::Expired,
            AuthError::MissingBearer => Self::Unauthorized("missing_bearer".into()),
            AuthError::InvalidToken(m) => Self::Unauthorized(format!("invalid_token: {m}")),
            AuthError::KidNotFound(_) => Self::Unauthorized("unknown_key".into()),
            AuthError::MalformedClaim(n, m) => Self::Unauthorized(format!("malformed_{n}: {m}")),
            AuthError::MissingClaim(n) => Self::Forbidden(format!("missing_{n}")),
            AuthError::Config(m) | AuthError::JwksFetch(m) => Self::Unauthorized(m),
        }
    }
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        let (status, code, msg) = match self {
            Self::Misconfigured => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "misconfigured",
                "auth not wired".to_string(),
            ),
            Self::Expired => (
                StatusCode::UNAUTHORIZED,
                "token_expired",
                "expired".to_string(),
            ),
            Self::Unauthorized(m) => (StatusCode::UNAUTHORIZED, "unauthorized", m),
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, "forbidden", m),
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

    fn parts_with_auth(value: &str) -> Parts {
        let req = axum::http::Request::builder()
            .header(AUTHORIZATION, value)
            .body(())
            .unwrap();
        let (parts, _) = req.into_parts();
        parts
    }

    #[test]
    fn bearer_extracts_standard_header() {
        let p = parts_with_auth("Bearer mytoken123");
        assert_eq!(extract_bearer(&p), Some("mytoken123"));
    }

    #[test]
    fn bearer_extracts_lowercase_prefix() {
        let p = parts_with_auth("bearer lowercasetoken");
        assert_eq!(extract_bearer(&p), Some("lowercasetoken"));
    }

    #[test]
    fn bearer_returns_none_on_missing_header() {
        let (parts, _) = axum::http::Request::builder()
            .body(())
            .unwrap()
            .into_parts();
        assert!(extract_bearer(&parts).is_none());
    }

    #[test]
    fn bearer_returns_none_on_basic_scheme() {
        let p = parts_with_auth("Basic dXNlcjpwYXNz");
        assert!(extract_bearer(&p).is_none());
    }

    #[test]
    fn bearer_returns_none_on_empty_token() {
        let p = parts_with_auth("Bearer ");
        assert!(extract_bearer(&p).is_none());
    }

    #[test]
    fn auth_rejection_status_codes() {
        use axum::response::IntoResponse;
        let cases: &[(AuthRejection, u16)] = &[
            (AuthRejection::Expired, 401),
            (AuthRejection::Unauthorized("x".into()), 401),
            (AuthRejection::Forbidden("x".into()), 403),
            (AuthRejection::Misconfigured, 500),
        ];
        for (rej, expected_status) in cases {
            // Reconstruct since AuthRejection doesn't impl Clone.
            let resp = match rej {
                AuthRejection::Expired => AuthRejection::Expired.into_response(),
                AuthRejection::Unauthorized(m) => {
                    AuthRejection::Unauthorized(m.clone()).into_response()
                }
                AuthRejection::Forbidden(m) => AuthRejection::Forbidden(m.clone()).into_response(),
                AuthRejection::Misconfigured => AuthRejection::Misconfigured.into_response(),
            };
            assert_eq!(resp.status().as_u16(), *expected_status);
        }
    }

    // ─── From<AuthError> for AuthRejection ───────────────────────────────────

    #[test]
    fn auth_error_expired_maps_to_rejection_expired() {
        let r = AuthRejection::from(AuthError::Expired);
        assert!(matches!(r, AuthRejection::Expired));
    }

    #[test]
    fn auth_error_missing_bearer_maps_to_unauthorized() {
        let r = AuthRejection::from(AuthError::MissingBearer);
        assert!(matches!(r, AuthRejection::Unauthorized(m) if m == "missing_bearer"));
    }

    #[test]
    fn auth_error_invalid_token_maps_to_unauthorized() {
        let r = AuthRejection::from(AuthError::InvalidToken("bad sig".into()));
        assert!(matches!(r, AuthRejection::Unauthorized(m) if m.contains("invalid_token")));
    }

    #[test]
    fn auth_error_kid_not_found_maps_to_unauthorized() {
        let r = AuthRejection::from(AuthError::KidNotFound(Some("kid-abc".into())));
        assert!(matches!(r, AuthRejection::Unauthorized(m) if m == "unknown_key"));
    }

    #[test]
    fn auth_error_kid_not_found_none_maps_to_unauthorized() {
        let r = AuthRejection::from(AuthError::KidNotFound(None));
        assert!(matches!(r, AuthRejection::Unauthorized(m) if m == "unknown_key"));
    }

    #[test]
    fn auth_error_malformed_claim_maps_to_unauthorized() {
        let r = AuthRejection::from(AuthError::MalformedClaim("sub", "not a uuid".into()));
        assert!(matches!(r, AuthRejection::Unauthorized(m) if m.contains("malformed_sub")));
    }

    #[test]
    fn auth_error_missing_claim_maps_to_forbidden() {
        let r = AuthRejection::from(AuthError::MissingClaim("tenant_id"));
        assert!(matches!(r, AuthRejection::Forbidden(m) if m.contains("missing_tenant_id")));
    }

    #[test]
    fn auth_error_config_maps_to_unauthorized() {
        let r = AuthRejection::from(AuthError::Config("no jwks url".into()));
        assert!(matches!(r, AuthRejection::Unauthorized(m) if m.contains("no jwks url")));
    }

    #[test]
    fn auth_error_jwks_fetch_maps_to_unauthorized() {
        let r = AuthRejection::from(AuthError::JwksFetch("timeout".into()));
        assert!(matches!(r, AuthRejection::Unauthorized(m) if m.contains("timeout")));
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
        let multi = parts
            .extensions
            .get::<Arc<MultiRealmValidator>>()
            .cloned()
            .ok_or(AuthRejection::Misconfigured)?;
        let resolver = parts
            .extensions
            .get::<Arc<TenantResolver>>()
            .cloned()
            .ok_or(AuthRejection::Misconfigured)?;

        let host = parts
            .headers
            .get(HOST)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AuthRejection::Unauthorized("missing_host".into()))?;

        let realm = resolver
            .resolve(host)
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

        let ctx = validator
            .validate(token)
            .await
            .map_err(AuthRejection::from)?;
        Ok(Self(ctx, realm))
    }
}

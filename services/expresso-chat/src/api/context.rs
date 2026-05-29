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

#[derive(Debug, Clone, Copy)]
pub struct RequestCtx {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
}

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for RequestCtx {
    type Rejection = CtxError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        // JWT mode: validator wired → Bearer required, strict validation.
        // Multi-realm precedence: if MultiRealmValidator + TenantResolver
        // present → resolve realm via Host header and validate.
        if let (Some(m), Some(r)) = (
            parts.extensions.get::<Arc<MultiRealmValidator>>().cloned(),
            parts.extensions.get::<Arc<TenantResolver>>().cloned(),
        ) {
            if let Some(host) = parts.headers.get(HOST).and_then(|v| v.to_str().ok()) {
                if let Some(realm) = r.resolve(host) {
                    let token_owned;
                    let token: &str = if let Some(t) = bearer_token(&parts.headers) {
                        t
                    } else if let Some(t) = cookie_token(&parts.headers, ACCESS_TOKEN_COOKIE) {
                        token_owned = t;
                        token_owned.as_str()
                    } else {
                        return Err(CtxError::MissingBearer);
                    };
                    let v = m.for_realm(realm).await.map_err(|e| {
                        auth_metrics::VALIDATION_TOTAL
                            .with_label_values(&[realm, auth_metrics::result_label(&e)])
                            .inc();
                        CtxError::from(e)
                    })?;
                    match v.validate(token).await {
                        Ok(c) => {
                            auth_metrics::VALIDATION_TOTAL
                                .with_label_values(&[realm, "ok"])
                                .inc();
                            return Ok(Self {
                                tenant_id: c.tenant_id,
                                user_id: c.user_id,
                            });
                        }
                        Err(e) => {
                            auth_metrics::VALIDATION_TOTAL
                                .with_label_values(&[realm, auth_metrics::result_label(&e)])
                                .inc();
                            return Err(CtxError::from(e));
                        }
                    }
                }
            }
        }

        if let Some(validator) = parts.extensions.get::<Arc<OidcValidator>>().cloned() {
            let token_owned;
            let token: &str = if let Some(t) = bearer_token(&parts.headers) {
                t
            } else if let Some(t) = cookie_token(&parts.headers, ACCESS_TOKEN_COOKIE) {
                token_owned = t;
                token_owned.as_str()
            } else {
                return Err(CtxError::MissingBearer);
            };
            let ctx = validator.validate(token).await.map_err(CtxError::from)?;
            return Ok(Self {
                tenant_id: ctx.tenant_id,
                user_id: ctx.user_id,
            });
        }

        // Header fallback (dev only).
        let tenant_id =
            parse_uuid_header(&parts.headers, H_TENANT).ok_or(CtxError::MissingHeader(H_TENANT))?;
        let user_id =
            parse_uuid_header(&parts.headers, H_USER).ok_or(CtxError::MissingHeader(H_USER))?;
        Ok(Self { tenant_id, user_id })
    }
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
    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    fn hmap(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            let name = HeaderName::from_bytes(k.as_bytes()).unwrap();
            m.insert(name, HeaderValue::from_str(v).unwrap());
        }
        m
    }

    #[test]
    fn parse_uuid_header_valid() {
        let id = "00000000-0000-0000-0000-000000000001";
        let h = hmap(&[("x-tenant-id", id)]);
        assert_eq!(
            parse_uuid_header(&h, "x-tenant-id").unwrap(),
            Uuid::parse_str(id).unwrap()
        );
    }

    #[test]
    fn parse_uuid_header_missing_key_returns_none() {
        let h = HeaderMap::new();
        assert!(parse_uuid_header(&h, "x-tenant-id").is_none());
    }

    #[test]
    fn parse_uuid_header_invalid_uuid_returns_none() {
        let h = hmap(&[("x-tenant-id", "not-a-uuid")]);
        assert!(parse_uuid_header(&h, "x-tenant-id").is_none());
    }

    #[test]
    fn parse_uuid_header_trims_whitespace() {
        let id = "00000000-0000-0000-0000-000000000002";
        let h = hmap(&[("x-tenant-id", &format!("  {id}  "))]);
        assert_eq!(
            parse_uuid_header(&h, "x-tenant-id").unwrap(),
            Uuid::parse_str(id).unwrap()
        );
    }

    #[test]
    fn bearer_token_extracts_from_bearer_prefix() {
        let h = hmap(&[("authorization", "Bearer mytoken123")]);
        assert_eq!(bearer_token(&h), Some("mytoken123"));
    }

    #[test]
    fn bearer_token_extracts_lowercase_bearer() {
        let h = hmap(&[("authorization", "bearer mytoken123")]);
        assert_eq!(bearer_token(&h), Some("mytoken123"));
    }

    #[test]
    fn bearer_token_missing_auth_header_returns_none() {
        let h = HeaderMap::new();
        assert!(bearer_token(&h).is_none());
    }

    #[test]
    fn bearer_token_empty_after_prefix_returns_none() {
        let h = hmap(&[("authorization", "Bearer  ")]);
        assert!(bearer_token(&h).is_none());
    }

    #[test]
    fn bearer_token_no_bearer_prefix_returns_none() {
        let h = hmap(&[("authorization", "Basic abc123")]);
        assert!(bearer_token(&h).is_none());
    }

    #[test]
    fn cookie_token_extracts_named_cookie() {
        let h = hmap(&[("cookie", "access_token=tok123; other=val")]);
        assert_eq!(cookie_token(&h, "access_token"), Some("tok123".into()));
    }

    #[test]
    fn cookie_token_missing_name_returns_none() {
        let h = hmap(&[("cookie", "other=val")]);
        assert!(cookie_token(&h, "access_token").is_none());
    }

    #[test]
    fn cookie_token_no_cookie_header_returns_none() {
        let h = HeaderMap::new();
        assert!(cookie_token(&h, "access_token").is_none());
    }

    #[test]
    fn cookie_token_empty_value_returns_none() {
        let h = hmap(&[("cookie", "access_token=")]);
        assert!(cookie_token(&h, "access_token").is_none());
    }

    #[test]
    fn ctx_error_missing_header_is_401() {
        let resp = CtxError::MissingHeader("x-tenant-id").into_response();
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn ctx_error_missing_bearer_is_401() {
        let resp = CtxError::MissingBearer.into_response();
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn ctx_error_invalid_token_is_401() {
        let resp = CtxError::InvalidToken("bad sig".into()).into_response();
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn ctx_error_expired_is_401() {
        let resp = CtxError::Expired.into_response();
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn ctx_error_forbidden_is_403() {
        let resp = CtxError::Forbidden("missing_role".into()).into_response();
        assert_eq!(resp.status().as_u16(), 403);
    }

    #[test]
    fn from_auth_error_expired_maps_to_expired() {
        assert!(matches!(
            CtxError::from(AuthError::Expired),
            CtxError::Expired
        ));
    }

    #[test]
    fn from_auth_error_missing_bearer_maps_correctly() {
        assert!(matches!(
            CtxError::from(AuthError::MissingBearer),
            CtxError::MissingBearer
        ));
    }

    #[test]
    fn from_auth_error_invalid_token_maps_correctly() {
        assert!(matches!(
            CtxError::from(AuthError::InvalidToken("x".into())),
            CtxError::InvalidToken(_)
        ));
    }

    #[test]
    fn from_auth_error_kid_not_found_maps_to_invalid_token() {
        assert!(matches!(
            CtxError::from(AuthError::KidNotFound(Some("kid".into()))),
            CtxError::InvalidToken(_)
        ));
    }

    #[test]
    fn from_auth_error_missing_claim_maps_to_forbidden() {
        assert!(matches!(
            CtxError::from(AuthError::MissingClaim("role")),
            CtxError::Forbidden(_)
        ));
    }

    #[test]
    fn from_auth_error_malformed_claim_maps_to_invalid_token() {
        assert!(matches!(
            CtxError::from(AuthError::MalformedClaim("sub", "not-uuid".into())),
            CtxError::InvalidToken(_)
        ));
    }

    #[test]
    fn parse_uuid_header_returns_correct_uuid_value() {
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let h = hmap(&[("x-user-id", id)]);
        let parsed = parse_uuid_header(&h, "x-user-id").unwrap();
        assert_eq!(parsed.to_string(), id);
    }

    #[test]
    fn h_tenant_constant_value() {
        assert_eq!(H_TENANT, "x-tenant-id");
    }

    #[test]
    fn h_user_constant_value() {
        assert_eq!(H_USER, "x-user-id");
    }
}

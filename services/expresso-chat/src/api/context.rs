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
    http::{header::{HeaderMap, AUTHORIZATION}, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use uuid::Uuid;

use expresso_auth_client::{AuthError, OidcValidator};

pub const H_TENANT: &str = "x-tenant-id";
pub const H_USER:   &str = "x-user-id";

#[derive(Debug, Clone, Copy)]
pub struct RequestCtx {
    pub tenant_id: Uuid,
    pub user_id:   Uuid,
}

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for RequestCtx {
    type Rejection = CtxError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        // JWT mode: validator wired → Bearer required, strict validation.
        if let Some(validator) = parts.extensions.get::<Arc<OidcValidator>>().cloned() {
            let token = bearer_token(&parts.headers).ok_or(CtxError::MissingBearer)?;
            let ctx = validator.validate(token).await.map_err(CtxError::from)?;
            return Ok(Self { tenant_id: ctx.tenant_id, user_id: ctx.user_id });
        }

        // Header fallback (dev only).
        let tenant_id = parse_uuid_header(&parts.headers, H_TENANT)
            .ok_or(CtxError::MissingHeader(H_TENANT))?;
        let user_id = parse_uuid_header(&parts.headers, H_USER)
            .ok_or(CtxError::MissingHeader(H_USER))?;
        Ok(Self { tenant_id, user_id })
    }
}

fn parse_uuid_header(h: &HeaderMap, name: &'static str) -> Option<Uuid> {
    h.get(name).and_then(|v| v.to_str().ok()).and_then(|s| Uuid::parse_str(s.trim()).ok())
}

fn bearer_token(h: &HeaderMap) -> Option<&str> {
    let raw = h.get(AUTHORIZATION)?.to_str().ok()?;
    let rest = raw.strip_prefix("Bearer ").or_else(|| raw.strip_prefix("bearer "))?;
    let t = rest.trim();
    if t.is_empty() { None } else { Some(t) }
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
            AuthError::Expired              => Self::Expired,
            AuthError::MissingBearer        => Self::MissingBearer,
            AuthError::InvalidToken(m)      => Self::InvalidToken(m),
            AuthError::KidNotFound(_)       => Self::InvalidToken("unknown_key".into()),
            AuthError::MalformedClaim(n, m) => Self::InvalidToken(format!("malformed_{n}: {m}")),
            AuthError::MissingClaim(n)      => Self::Forbidden(format!("missing_{n}")),
            AuthError::Config(m) | AuthError::JwksFetch(m) => Self::InvalidToken(m),
        }
    }
}

impl IntoResponse for CtxError {
    fn into_response(self) -> Response {
        let (status, code, msg) = match self {
            Self::MissingHeader(h) => (StatusCode::UNAUTHORIZED, "missing_header", h.to_string()),
            Self::MissingBearer    => (StatusCode::UNAUTHORIZED, "missing_bearer", "Authorization: Bearer <jwt> required".into()),
            Self::InvalidToken(m)  => (StatusCode::UNAUTHORIZED, "invalid_token",  m),
            Self::Expired          => (StatusCode::UNAUTHORIZED, "token_expired",  "expired".into()),
            Self::Forbidden(m)     => (StatusCode::FORBIDDEN,    "forbidden",      m),
        };
        (status, Json(json!({"error": code, "message": msg}))).into_response()
    }
}

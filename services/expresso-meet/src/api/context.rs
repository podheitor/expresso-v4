//! Request context extractor — dual-mode (JWT via OIDC, header fallback).
//!
//! Mode 1 (strict): `Arc<OidcValidator>` present in extensions → require
//! `Authorization: Bearer …`, validate, derive tenant/user/name/email.
//! Mode 2 (dev fallback): read `X-Tenant-Id` / `X-User-Id` / `X-User-Name` /
//! `X-User-Email` when no validator is wired.
//!
//! Display name / email feed the Jitsi JWT (`context.user.name|.email`) —
//! fallbacks keep dev e2e flows working without auth.

use std::sync::Arc;

use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{header::{HeaderMap, AUTHORIZATION, COOKIE}, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use uuid::Uuid;

use expresso_auth_client::{AuthError, OidcValidator, ACCESS_TOKEN_COOKIE};

pub const H_TENANT: &str = "x-tenant-id";
pub const H_USER:   &str = "x-user-id";
pub const H_NAME:   &str = "x-user-name";
pub const H_EMAIL:  &str = "x-user-email";

#[derive(Debug, Clone)]
pub struct RequestCtx {
    pub tenant_id:    Uuid,
    pub user_id:      Uuid,
    pub display_name: String,
    pub email:        String,
}

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for RequestCtx {
    type Rejection = CtxError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
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
                tenant_id:    ctx.tenant_id,
                user_id:      ctx.user_id,
                display_name: ctx.display_name,
                email:        ctx.email,
            });
        }

        // Header fallback (dev).
        let tenant_id = parse_uuid_header(&parts.headers, H_TENANT)
            .ok_or(CtxError::MissingHeader(H_TENANT))?;
        let user_id = parse_uuid_header(&parts.headers, H_USER)
            .ok_or(CtxError::MissingHeader(H_USER))?;
        let display_name = parts.headers.get(H_NAME)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| format!("user-{}", &user_id.to_string()[..8]));
        let email = parts.headers.get(H_EMAIL)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        Ok(Self { tenant_id, user_id, display_name, email })
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


fn cookie_token(h: &HeaderMap, name: &str) -> Option<String> {
    for hv in h.get_all(COOKIE).iter() {
        let s = match hv.to_str() { Ok(v) => v, Err(_) => continue };
        for pair in s.split(';') {
            let pair = pair.trim();
            if let Some((k, v)) = pair.split_once('=') {
                if k.trim() == name {
                    let v = v.trim();
                    if !v.is_empty() { return Some(v.to_string()); }
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

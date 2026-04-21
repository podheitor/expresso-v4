//! Axum extractor: `Authenticated(AuthContext)`.
//!
//! Reads `Authorization: Bearer <jwt>`, validates via `OidcValidator`
//! (fetched from the request's `Extensions`), returns a concrete
//! `AuthContext` or an HTTP 401/403 response.

use std::sync::Arc;

use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{header::AUTHORIZATION, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::claims::AuthContext;
use crate::error::AuthError;
use crate::validator::OidcValidator;

pub struct Authenticated(pub AuthContext);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for Authenticated {
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let validator = parts.extensions
            .get::<Arc<OidcValidator>>()
            .cloned()
            .ok_or(AuthRejection::Misconfigured)?;

        let token = extract_bearer(parts)
            .ok_or(AuthRejection::from(AuthError::MissingBearer))?;

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

//! Request context extractor — tenant + user + optional display metadata.
//!
//! Display name / email come from the auth proxy (Keycloak → gateway →
//! headers). Required for Jitsi JWT (`context.user.name` / `.email`) —
//! defaults kick in when absent (dev/staging without auth bridge).

use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{header::HeaderMap, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use uuid::Uuid;

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

#[derive(Debug)]
pub enum CtxError { MissingHeader(&'static str) }

impl IntoResponse for CtxError {
    fn into_response(self) -> Response {
        let Self::MissingHeader(h) = self;
        (StatusCode::UNAUTHORIZED,
         Json(json!({"error":"missing_header","header":h}))).into_response()
    }
}

//! Per-request identity — header-based MVP (mirrors expresso-contacts).
//! Upstream gateway sets X-Tenant-Id / X-User-Id after validating JWT.

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

#[derive(Debug, Clone, Copy)]
pub struct RequestCtx {
    pub tenant_id: Uuid,
    pub user_id:   Uuid,
}

#[async_trait]
impl<S> FromRequestParts<S> for RequestCtx
where
    S: Send + Sync,
{
    type Rejection = CtxError;
    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let tenant_id = hdr(&parts.headers, H_TENANT).ok_or(CtxError::MissingHeader(H_TENANT))?;
        let user_id   = hdr(&parts.headers, H_USER).ok_or(CtxError::MissingHeader(H_USER))?;
        Ok(Self { tenant_id, user_id })
    }
}

fn hdr(h: &HeaderMap, k: &str) -> Option<Uuid> {
    h.get(k).and_then(|v| v.to_str().ok()).and_then(|s| Uuid::parse_str(s.trim()).ok())
}

#[derive(Debug)]
pub enum CtxError {
    MissingHeader(&'static str),
}

impl IntoResponse for CtxError {
    fn into_response(self) -> Response {
        let msg = match self { CtxError::MissingHeader(h) => format!("missing header {h}") };
        (StatusCode::UNAUTHORIZED, Json(json!({ "error": msg }))).into_response()
    }
}

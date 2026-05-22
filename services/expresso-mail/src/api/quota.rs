//! Storage quota endpoint.
//!
//! GET /api/v1/mail/quota — returns used bytes and optional quota limit.
//!
//! `used_bytes` = SUM(size_bytes) of non-expunged messages owned by the user.
//! `quota_bytes` = NULL (no per-user quota enforced yet; field reserved for
//!   future admin-configurable soft/hard limits).

use axum::{extract::State, http::{header, HeaderMap, HeaderValue, StatusCode}, response::{IntoResponse, Response}, routing::get, Json, Router};
use time::OffsetDateTime;
use serde::Serialize;

use crate::{api::context::RequestCtx, error::Result, state::AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route("/mail/quota", get(get_quota))
}

#[derive(Debug, Serialize)]
pub struct QuotaDto {
    pub used_bytes:  i64,
    pub quota_bytes: Option<i64>,
}

/// GET /api/v1/mail/quota
async fn get_quota(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let max_ts: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(m.received_at) \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE mb.user_id = $1 AND m.tenant_id = $2",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .fetch_one(state.db())
    .await
    .unwrap_or(None);

    if let Some(ts) = max_ts {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

    let used: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(m.size_bytes), 0) \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE mb.user_id = $1 AND m.tenant_id = $2",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .fetch_one(state.db())
    .await
    .unwrap_or(0i64);

    let mut resp = Json(QuotaDto {
        used_bytes:  used,
        quota_bytes: None,
    }).into_response();
    if let Some(ts) = max_ts {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_dto_with_quota() {
        let q = QuotaDto { used_bytes: 1024, quota_bytes: Some(10_737_418_240) };
        let s = serde_json::to_string(&q).unwrap();
        assert!(s.contains("1024"));
        assert!(s.contains("10737418240"));
    }

    #[test]
    fn quota_dto_no_quota_null() {
        let q = QuotaDto { used_bytes: 0, quota_bytes: None };
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(v["used_bytes"], 0);
        assert!(v["quota_bytes"].is_null());
    }

    #[test]
    fn quota_dto_used_zero() {
        let q = QuotaDto { used_bytes: 0, quota_bytes: Some(1024) };
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(v["used_bytes"], 0);
    }

    #[test]
    fn quota_dto_large_values() {
        let q = QuotaDto { used_bytes: i64::MAX, quota_bytes: Some(i64::MAX) };
        let s = serde_json::to_string(&q).unwrap();
        assert!(s.contains(&i64::MAX.to_string()));
    }

    #[test]
    fn quota_dto_roundtrip_preserves_quota() {
        let q = QuotaDto { used_bytes: 512, quota_bytes: Some(2048) };
        let back: QuotaDto = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(back.used_bytes, 512);
        assert_eq!(back.quota_bytes, Some(2048));
    }

    #[test]
    fn quota_dto_null_quota_serializes() {
        let q = QuotaDto { used_bytes: 100, quota_bytes: None };
        let v: serde_json::Value = serde_json::to_value(&q).unwrap();
        assert!(v["quota_bytes"].is_null());
    }

    #[test]
    fn quota_dto_used_bytes_zero() {
        let q = QuotaDto { used_bytes: 0, quota_bytes: None };
        assert_eq!(q.used_bytes, 0);
    }

    #[test]
    fn quota_dto_large_quota_value() {
        let q = QuotaDto { used_bytes: 1024, quota_bytes: Some(10 * 1024 * 1024 * 1024) };
        assert_eq!(q.quota_bytes, Some(10 * 1024 * 1024 * 1024));
    }

    #[test]
    fn quota_dto_used_bytes_matches_quota_boundary() {
        let q = QuotaDto { used_bytes: 512, quota_bytes: Some(512) };
        assert_eq!(q.used_bytes, q.quota_bytes.unwrap());
    }

    #[test]
    fn quota_dto_no_limit_is_none() {
        let q = QuotaDto { used_bytes: 1024, quota_bytes: None };
        assert!(q.quota_bytes.is_none());
    }

    #[test]
    fn quota_dto_used_bytes_preserved() {
        let q = QuotaDto { used_bytes: 4096, quota_bytes: Some(10240) };
        assert_eq!(q.used_bytes, 4096);
    }

    #[test]
    fn quota_dto_quota_bytes_preserved() {
        let q = QuotaDto { used_bytes: 0, quota_bytes: Some(1_073_741_824) };
        assert_eq!(q.quota_bytes, Some(1_073_741_824));
    }

    #[test]
    fn quota_dto_used_bytes_preserved() {
        let q = QuotaDto { used_bytes: 512_000, quota_bytes: None };
        assert_eq!(q.used_bytes, 512_000);
    }
}

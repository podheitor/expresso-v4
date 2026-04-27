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

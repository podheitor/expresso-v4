//! Storage quota endpoint.
//!
//! GET /api/v1/mail/quota — returns used bytes and optional quota limit.
//!
//! `used_bytes` = SUM(size_bytes) of non-expunged messages owned by the user.
//! `quota_bytes` = NULL (no per-user quota enforced yet; field reserved for
//!   future admin-configurable soft/hard limits).

use axum::{extract::State, routing::get, Json, Router};
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
    ctx: RequestCtx,
) -> Result<Json<QuotaDto>> {
    let used: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(size_bytes), 0) \
         FROM messages \
         WHERE user_id = $1 AND tenant_id = $2 AND expunged_at IS NULL",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .fetch_one(state.db())
    .await
    .unwrap_or(0i64);

    Ok(Json(QuotaDto {
        used_bytes:  used,
        quota_bytes: None,
    }))
}

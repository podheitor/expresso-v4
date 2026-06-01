//! Flow webhook delivery log (read-only).
//!
//! GET /api/v1/mail/flows/webhook-log[?limit=] — the caller's recent webhook
//! deliveries (from flow `webhook` actions executed during ingest), newest
//! first, with each attempt's HTTP status / error. Lets a user debug failing
//! automations. Writes happen in the ingest path (`apply_flow_actions`).

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::Result;
use crate::state::AppState;
use expresso_core::begin_tenant_tx;

/// Default and max rows returned.
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

pub fn routes() -> Router<AppState> {
    Router::new().route("/mail/flows/webhook-log", get(list_webhook_log))
}

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct WebhookLogEntry {
    pub id: Uuid,
    pub message_id: Option<Uuid>,
    pub url: String,
    pub status_code: Option<i32>,
    pub ok: bool,
    pub error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

async fn list_webhook_log(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Query(q): Query<LogQuery>,
) -> Result<Json<Vec<WebhookLogEntry>>> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let rows = sqlx::query_as::<_, WebhookLogEntry>(
        "SELECT id, message_id, url, status_code, ok, error, created_at \
           FROM flow_webhook_log \
          WHERE tenant_id = $1 AND user_id = $2 \
          ORDER BY created_at DESC LIMIT $3",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

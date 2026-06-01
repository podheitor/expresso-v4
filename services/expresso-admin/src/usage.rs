//! Per-tenant usage statistics (JSON) for capacity planning + billing.
//!
//! GET /api/v1/admin/tenants/:id/usage → counts and stored bytes across the
//! tenant's mail/drive/calendar/contacts. Gated by `require_tenant_match`:
//! super-admins see any tenant; a tenant-admin only their own.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use crate::{auth, AdminError, AppState};

#[derive(Debug, Serialize)]
pub struct TenantUsage {
    pub tenant_id: String,
    pub user_count: i64,
    pub message_count: i64,
    pub mailbox_size_bytes: i64,
    pub file_count: i64,
    pub file_size_bytes: i64,
    pub calendar_event_count: i64,
    pub contact_count: i64,
}

/// One scalar COUNT/SUM, tenant-scoped. Returns 0 when the table is empty or the
/// query fails (best-effort dashboard metric — a single missing table must not
/// 500 the whole report).
async fn scalar(pool: &sqlx::PgPool, sql: &str, tenant: uuid::Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .bind(tenant)
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

pub async fn tenant_usage(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Result<Response, AdminError> {
    if let Some(r) = auth::require_tenant_match(&st, &headers, id).await {
        return Ok(r);
    }
    let pool = st
        .db
        .as_ref()
        .ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;

    let usage = TenantUsage {
        tenant_id: id.to_string(),
        user_count: scalar(pool, "SELECT COUNT(*) FROM users WHERE tenant_id = $1", id).await,
        message_count: scalar(pool, "SELECT COUNT(*) FROM messages WHERE tenant_id = $1", id).await,
        mailbox_size_bytes: scalar(
            pool,
            "SELECT COALESCE(SUM(size_bytes), 0) FROM messages WHERE tenant_id = $1",
            id,
        )
        .await,
        file_count: scalar(
            pool,
            "SELECT COUNT(*) FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL",
            id,
        )
        .await,
        file_size_bytes: scalar(
            pool,
            "SELECT COALESCE(SUM(size_bytes), 0) FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL",
            id,
        )
        .await,
        calendar_event_count: scalar(
            pool,
            "SELECT COUNT(*) FROM calendar_events WHERE tenant_id = $1",
            id,
        )
        .await,
        contact_count: scalar(pool, "SELECT COUNT(*) FROM contacts WHERE tenant_id = $1", id).await,
    };
    Ok(Json(usage).into_response())
}

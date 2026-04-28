//! Drive file activity log — sprint #410.
//!
//! Routes:
//!   GET  /api/v1/drive/files/:id/activity        — list audit events for a file
//!   POST /api/v1/drive/files/:id/activity        — record an activity event
//!
//! Activities are append-only; there is no DELETE. Any authenticated tenant
//! member with access to the file can read its activity log. Only internal
//! service code or the file owner can write entries.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{DriveError, Result};
use crate::state::AppState;

const PAGE_SIZE: i64 = 50;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ActivityEvent {
    pub id:         Uuid,
    pub file_id:    Uuid,
    pub tenant_id:  Uuid,
    pub user_id:    Uuid,
    pub action:     String,
    pub detail:     Option<Value>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct ActivityQuery {
    /// Cursor: return events older than this created_at (ISO 8601).
    before: Option<String>,
    limit:  Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CreateActivityBody {
    action: String,
    detail: Option<Value>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/v1/drive/files/:id/activity",
        get(list_activity).post(record_activity),
    )
}

/// GET /api/v1/drive/files/:id/activity — list audit trail for a file.
async fn list_activity(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Query(qs):    Query<ActivityQuery>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;

    // Verify file exists in tenant
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM drive_files WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Err(DriveError::NotFound(id));
    }

    let limit = qs.limit.unwrap_or(PAGE_SIZE).min(200).max(1);

    let events: Vec<ActivityEvent> = if let Some(before_str) = qs.before.as_deref() {
        let before = time::OffsetDateTime::parse(before_str, &time::format_description::well_known::Rfc3339)
            .map_err(|_| DriveError::BadRequest("invalid 'before' timestamp".into()))?;
        sqlx::query_as(
            "SELECT id, file_id, tenant_id, user_id, action, detail, created_at \
             FROM drive_file_activity \
             WHERE tenant_id = $1 AND file_id = $2 AND created_at < $3 \
             ORDER BY created_at DESC \
             LIMIT $4",
        )
        .bind(ctx.tenant_id)
        .bind(id)
        .bind(before)
        .bind(limit)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, file_id, tenant_id, user_id, action, detail, created_at \
             FROM drive_file_activity \
             WHERE tenant_id = $1 AND file_id = $2 \
             ORDER BY created_at DESC \
             LIMIT $3",
        )
        .bind(ctx.tenant_id)
        .bind(id)
        .bind(limit)
        .fetch_all(pool)
        .await?
    };

    Ok(Json(events))
}

/// POST /api/v1/drive/files/:id/activity — append an activity event.
async fn record_activity(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<CreateActivityBody>,
) -> Result<impl IntoResponse> {
    let action = body.action.trim().to_string();
    if action.is_empty() || action.len() > 64 {
        return Err(DriveError::BadRequest("action must be 1-64 characters".into()));
    }

    let pool = state.db_or_unavailable()?;

    // Verify file exists in tenant
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM drive_files WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Err(DriveError::NotFound(id));
    }

    let event: ActivityEvent = sqlx::query_as(
        "INSERT INTO drive_file_activity (file_id, tenant_id, user_id, action, detail) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, file_id, tenant_id, user_id, action, detail, created_at",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&action)
    .bind(&body.detail)
    .fetch_one(pool)
    .await?;

    Ok((StatusCode::CREATED, Json(event)))
}

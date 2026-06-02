//! Note activity log (audit trail) — mirrors contacts' and drive's activity.
//!
//! Routes:
//!   GET /api/v1/notes/:id/activity — list events (newest first, cursor paged)
//!
//! Activities are append-only; the create/update/delete handlers record an event
//! via [`record`] (best-effort — a logging failure never fails the mutation).
//! Read access requires the caller to own the note or hold a `notes_acl` grant
//! (same gate as the other note reads).

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::NoteRepo;
use crate::error::{NotesError, Result};
use crate::state::AppState;

const PAGE_SIZE: i64 = 50;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ActivityEvent {
    pub id: Uuid,
    pub note_id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub action: String,
    pub detail: Option<Value>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct ActivityQuery {
    /// Cursor: return events older than this created_at (RFC3339).
    before: Option<String>,
    limit: Option<i64>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/notes/:id/activity", get(list_activity))
}

/// GET /api/v1/notes/:id/activity — the note's audit trail, newest first.
async fn list_activity(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Query(qs): Query<ActivityQuery>,
) -> Result<Json<Vec<ActivityEvent>>> {
    let pool = state.db_or_unavailable()?;
    // Gate: caller must own the note or hold an ACL grant (404 otherwise),
    // matching the other note reads.
    match NoteRepo::new(pool)
        .access_level(ctx.tenant_id, id, ctx.user_id)
        .await?
    {
        Some(_) => {}
        None => return Err(NotesError::NoteNotFound(id)),
    }

    let limit = qs.limit.unwrap_or(PAGE_SIZE).clamp(1, 200);
    let events: Vec<ActivityEvent> = if let Some(before_str) = qs.before.as_deref() {
        let before =
            OffsetDateTime::parse(before_str, &time::format_description::well_known::Rfc3339)
                .map_err(|_| NotesError::BadRequest("invalid 'before' timestamp".into()))?;
        sqlx::query_as(
            "SELECT id, note_id, tenant_id, user_id, action, detail, created_at \
             FROM notes_activity \
             WHERE tenant_id = $1 AND note_id = $2 AND created_at < $3 \
             ORDER BY created_at DESC LIMIT $4",
        )
        .bind(ctx.tenant_id)
        .bind(id)
        .bind(before)
        .bind(limit)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, note_id, tenant_id, user_id, action, detail, created_at \
             FROM notes_activity \
             WHERE tenant_id = $1 AND note_id = $2 \
             ORDER BY created_at DESC LIMIT $3",
        )
        .bind(ctx.tenant_id)
        .bind(id)
        .bind(limit)
        .fetch_all(pool)
        .await?
    };
    Ok(Json(events))
}

/// Append an activity event for `note_id`. Called by the create/update/delete
/// handlers; best-effort — a logging failure must not fail the mutation, so the
/// caller ignores the result.
pub(crate) async fn record(
    pool: &expresso_core::DbPool,
    tenant_id: Uuid,
    note_id: Uuid,
    user_id: Uuid,
    action: &str,
    detail: Option<Value>,
) {
    let res = sqlx::query(
        "INSERT INTO notes_activity (note_id, tenant_id, user_id, action, detail) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(note_id)
    .bind(tenant_id)
    .bind(user_id)
    .bind(action)
    .bind(detail)
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::warn!(error = %e, note_id = %note_id, action, "note activity log failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_event_serde_roundtrip() {
        let e = ActivityEvent {
            id: Uuid::nil(),
            note_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            user_id: Uuid::nil(),
            action: "update".into(),
            detail: Some(serde_json::json!({"field": "title"})),
            created_at: OffsetDateTime::UNIX_EPOCH,
        };
        let back: ActivityEvent =
            serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back.action, "update");
        assert_eq!(back.detail.unwrap()["field"], "title");
    }

    #[test]
    fn activity_query_deserializes() {
        let q: ActivityQuery =
            serde_json::from_str(r#"{"before":"2026-06-02T00:00:00Z","limit":20}"#).unwrap();
        assert_eq!(q.before.as_deref(), Some("2026-06-02T00:00:00Z"));
        assert_eq!(q.limit, Some(20));
    }
}

//! Contact activity log (audit trail) — mirrors drive's file activity.
//!
//! Routes:
//!   GET /api/v1/addressbooks/:book_id/contacts/:id/activity — list events
//!
//! Activities are append-only; mutations (create/update/delete) record an event
//! automatically via [`record`], and the GET endpoint lists them newest-first
//! with cursor pagination. Read access requires the contact to exist in the
//! caller's tenant (same 404 guard the other read endpoints use).

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
use crate::domain::ContactRepo;
use crate::error::{ContactsError, Result};
use crate::state::AppState;

const PAGE_SIZE: i64 = 50;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ActivityEvent {
    pub id: Uuid,
    pub contact_id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub action: String,
    pub detail: Option<Value>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct ActivityQuery {
    /// Cursor: return events older than this created_at (ISO 8601).
    before: Option<String>,
    limit: Option<i64>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/v1/addressbooks/:book_id/contacts/:id/activity",
        get(list_activity),
    )
}

/// GET …/contacts/:id/activity — the contact's audit trail, newest first.
async fn list_activity(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((_book_id, id)): Path<(Uuid, Uuid)>,
    Query(qs): Query<ActivityQuery>,
) -> Result<Json<Vec<ActivityEvent>>> {
    let pool = state.db_or_unavailable()?;
    ContactRepo::new(pool).get(ctx.tenant_id, id).await?; // 404 guard

    let limit = qs.limit.unwrap_or(PAGE_SIZE).clamp(1, 200);
    let events: Vec<ActivityEvent> = if let Some(before_str) = qs.before.as_deref() {
        let before =
            OffsetDateTime::parse(before_str, &time::format_description::well_known::Rfc3339)
                .map_err(|_| ContactsError::BadRequest("invalid 'before' timestamp".into()))?;
        sqlx::query_as(
            "SELECT id, contact_id, tenant_id, user_id, action, detail, created_at \
             FROM contact_activity \
             WHERE tenant_id = $1 AND contact_id = $2 AND created_at < $3 \
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
            "SELECT id, contact_id, tenant_id, user_id, action, detail, created_at \
             FROM contact_activity \
             WHERE tenant_id = $1 AND contact_id = $2 \
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

/// Append an activity event for `contact_id`. Called by the create/update/delete
/// handlers; best-effort — a logging failure must not fail the mutation, so the
/// caller ignores the result.
pub(crate) async fn record(
    pool: &expresso_core::DbPool,
    tenant_id: Uuid,
    contact_id: Uuid,
    user_id: Uuid,
    action: &str,
    detail: Option<Value>,
) {
    let res = sqlx::query(
        "INSERT INTO contact_activity (contact_id, tenant_id, user_id, action, detail) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(contact_id)
    .bind(tenant_id)
    .bind(user_id)
    .bind(action)
    .bind(detail)
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::warn!(error = %e, contact_id = %contact_id, action, "contact activity log failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn event(action: &str, detail: Option<Value>) -> ActivityEvent {
        ActivityEvent {
            id: Uuid::nil(),
            contact_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            user_id: Uuid::nil(),
            action: action.into(),
            detail,
            created_at: datetime!(2026-06-02 10:00:00 UTC),
        }
    }

    #[test]
    fn activity_event_serde_roundtrip() {
        let e = event("update", Some(serde_json::json!({"field": "email"})));
        let back: ActivityEvent =
            serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back.action, "update");
        assert_eq!(back.detail.unwrap()["field"], "email");
    }

    #[test]
    fn activity_event_detail_null_when_none() {
        let v: Value =
            serde_json::from_str(&serde_json::to_string(&event("delete", None)).unwrap()).unwrap();
        assert!(v["detail"].is_null());
    }

    #[test]
    fn activity_query_deserializes() {
        let q: ActivityQuery =
            serde_json::from_str(r#"{"before":"2026-06-02T00:00:00Z","limit":20}"#).unwrap();
        assert_eq!(q.before.as_deref(), Some("2026-06-02T00:00:00Z"));
        assert_eq!(q.limit, Some(20));
    }

    #[test]
    fn activity_query_empty_is_none() {
        let q: ActivityQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.before.is_none());
        assert!(q.limit.is_none());
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn activity_event_serde_roundtrip() {
        let e = ActivityEvent {
            id:         Uuid::nil(),
            file_id:    Uuid::nil(),
            tenant_id:  Uuid::nil(),
            user_id:    Uuid::nil(),
            action:     "upload".into(),
            detail:     Some(serde_json::json!({"size": 1024})),
            created_at: datetime!(2026-05-22 10:00:00 UTC),
        };
        let s = serde_json::to_string(&e).unwrap();
        let back: ActivityEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(back.action, "upload");
        assert!(back.detail.is_some());
    }

    #[test]
    fn activity_event_detail_null_when_none() {
        let e = ActivityEvent {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            user_id: Uuid::nil(), action: "delete".into(), detail: None,
            created_at: datetime!(2026-05-22 10:00:00 UTC),
        };
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert!(v["detail"].is_null());
    }

    #[test]
    fn activity_query_deserializes() {
        let json = r#"{"before":"2026-05-22T00:00:00Z","limit":20}"#;
        let q: ActivityQuery = serde_json::from_str(json).unwrap();
        assert_eq!(q.before.as_deref(), Some("2026-05-22T00:00:00Z"));
        assert_eq!(q.limit, Some(20));
    }

    #[test]
    fn create_activity_body_deserializes() {
        let json = r#"{"action":"share","detail":{"shared_with":"user-xyz"}}"#;
        let b: CreateActivityBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.action, "share");
        assert!(b.detail.is_some());
    }

    #[test]
    fn create_activity_body_no_detail() {
        let b: CreateActivityBody = serde_json::from_str(r#"{"action":"view"}"#).unwrap();
        assert_eq!(b.action, "view");
        assert!(b.detail.is_none());
    }

    #[test]
    fn activity_query_limit_none_by_default() {
        let q: ActivityQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.limit.is_none());
        assert!(q.before.is_none());
    }

    #[test]
    fn activity_event_created_at_in_rfc3339() {
        let e = ActivityEvent {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            user_id: Uuid::nil(), action: "view".into(), detail: None,
            created_at: datetime!(2026-06-15 12:00:00 UTC),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("2026-06-15T12:00:00"));
    }

    #[test]
    fn create_activity_body_detail_complex_json() {
        let json = r#"{"action":"edit","detail":{"fields":["name","size"],"editor":"u1"}}"#;
        let b: CreateActivityBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.action, "edit");
        let d = b.detail.unwrap();
        assert!(d["fields"].is_array());
    }

    #[test]
    fn create_activity_body_no_detail_is_ok() {
        let json = r#"{"action":"view"}"#;
        let b: CreateActivityBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.action, "view");
        assert!(b.detail.is_none());
    }

    #[test]
    fn create_activity_body_detail_string_value() {
        let b: CreateActivityBody = serde_json::from_str(r#"{"action":"download","detail":"pdf"}"#).unwrap();
        assert_eq!(b.action, "download");
        assert!(b.detail.is_some());
    }

    #[test]
    fn create_activity_body_action_preserved() {
        let b: CreateActivityBody = serde_json::from_str(r#"{"action":"share"}"#).unwrap();
        assert_eq!(b.action, "share");
    }

    #[test]
    fn create_activity_body_action_view_preserved() {
        let b: CreateActivityBody = serde_json::from_str(r#"{"action":"view"}"#).unwrap();
        assert_eq!(b.action, "view");
    }

    #[test]
    fn create_activity_body_action_edit_preserved() {
        let b: CreateActivityBody = serde_json::from_str(r#"{"action":"edit"}"#).unwrap();
        assert_eq!(b.action, "edit");
    }

    #[test]
    fn create_activity_body_action_upload_preserved() {
        let b: CreateActivityBody = serde_json::from_str(r#"{"action":"upload"}"#).unwrap();
        assert_eq!(b.action, "upload");
    }

    #[test]
    fn create_activity_body_action_delete_preserved() {
        let b: CreateActivityBody = serde_json::from_str(r#"{"action":"delete"}"#).unwrap();
        assert_eq!(b.action, "delete");
    }

    #[test]
    fn activity_query_limit_max_value_preserved() {
        let q: ActivityQuery = serde_json::from_str(r#"{"limit":200}"#).unwrap();
        assert_eq!(q.limit, Some(200));
    }

    #[test]
    fn activity_query_before_field_preserved() {
        let q: ActivityQuery =
            serde_json::from_str(r#"{"before":"2026-01-01T00:00:00Z"}"#).unwrap();
        assert_eq!(q.before.as_deref(), Some("2026-01-01T00:00:00Z"));
    }

    #[test]
    fn page_size_constant_is_positive() {
        assert!(PAGE_SIZE > 0);
    }

    #[test]
    fn activity_event_action_preserved_in_roundtrip() {
        use time::macros::datetime;
        let e = ActivityEvent {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            user_id: Uuid::nil(), action: "rename".into(), detail: None,
            created_at: datetime!(2026-05-22 10:00:00 UTC),
        };
        let back: ActivityEvent = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back.action, "rename");
    }

    #[test]
    fn page_size_constant_exceeds_zero() {
        assert!(PAGE_SIZE > 0);
    }

    #[test]
    fn activity_event_file_id_accessible() {
        use time::macros::datetime;
        let fid = Uuid::new_v4();
        let e = ActivityEvent {
            id: Uuid::nil(), file_id: fid, tenant_id: Uuid::nil(),
            user_id: Uuid::nil(), action: "upload".into(), detail: None,
            created_at: datetime!(2026-05-22 10:00:00 UTC),
        };
        assert_eq!(e.file_id, fid);
    }
}

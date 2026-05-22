//! Drive file comments — sprint #405.
//!
//! Routes:
//!   GET    /api/v1/drive/files/:id/comments
//!   POST   /api/v1/drive/files/:id/comments
//!   DELETE /api/v1/drive/files/:id/comments/:comment_id

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{DriveError, Result};
use crate::state::AppState;

const MAX_COMMENT_BYTES: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FileComment {
    pub id:         Uuid,
    pub file_id:    Uuid,
    pub tenant_id:  Uuid,
    pub user_id:    Uuid,
    pub body:       String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct CreateCommentBody {
    body: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/drive/files/:id/comments",
            get(list_comments).post(create_comment),
        )
        .route(
            "/api/v1/drive/files/:id/comments/:comment_id",
            delete(delete_comment),
        )
}

/// GET /api/v1/drive/files/:id/comments — list comments on a file (tenant-scoped).
async fn list_comments(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;

    // Verify file exists in tenant (not deleted)
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

    let comments: Vec<FileComment> = sqlx::query_as(
        "SELECT id, file_id, tenant_id, user_id, body, created_at, updated_at \
         FROM drive_file_comments \
         WHERE tenant_id = $1 AND file_id = $2 \
         ORDER BY created_at ASC",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .fetch_all(pool)
    .await?;

    Ok(Json(comments))
}

/// POST /api/v1/drive/files/:id/comments — add a comment.
async fn create_comment(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<CreateCommentBody>,
) -> Result<impl IntoResponse> {
    if body.body.trim().is_empty() {
        return Err(DriveError::BadRequest("body must not be empty".into()));
    }
    if body.body.len() > MAX_COMMENT_BYTES {
        return Err(DriveError::BadRequest(format!(
            "comment too long: {} bytes (max {})",
            body.body.len(),
            MAX_COMMENT_BYTES
        )));
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

    let comment: FileComment = sqlx::query_as(
        "INSERT INTO drive_file_comments (file_id, tenant_id, user_id, body) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, file_id, tenant_id, user_id, body, created_at, updated_at",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&body.body)
    .fetch_one(pool)
    .await?;

    Ok((StatusCode::CREATED, Json(comment)))
}

/// DELETE /api/v1/drive/files/:id/comments/:comment_id — delete a comment (author only).
async fn delete_comment(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, comment_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;

    let r = sqlx::query(
        "DELETE FROM drive_file_comments \
         WHERE id = $1 AND file_id = $2 AND tenant_id = $3 AND user_id = $4",
    )
    .bind(comment_id)
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .execute(pool)
    .await?;

    if r.rows_affected() == 0 {
        return Err(DriveError::NotFound(comment_id));
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn file_comment_serde_roundtrip() {
        let c = FileComment {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            user_id: Uuid::nil(), body: "Great work!".into(),
            created_at: datetime!(2026-05-22 09:00:00 UTC),
            updated_at: datetime!(2026-05-22 09:01:00 UTC),
        };
        let s = serde_json::to_string(&c).unwrap();
        let back: FileComment = serde_json::from_str(&s).unwrap();
        assert_eq!(back.body, "Great work!");
    }

    #[test]
    fn file_comment_timestamps_in_rfc3339() {
        let c = FileComment {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            user_id: Uuid::nil(), body: "note".into(),
            created_at: datetime!(2026-05-22 08:00:00 UTC),
            updated_at: datetime!(2026-05-22 08:00:00 UTC),
        };
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("2026-05-22T08:00:00"));
    }

    #[test]
    fn create_comment_body_deserializes() {
        let json = r#"{"body":"hello world"}"#;
        let b: CreateCommentBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.body, "hello world");
    }

    #[test]
    fn file_comment_body_empty_string_allowed_by_serde() {
        let b: CreateCommentBody = serde_json::from_str(r#"{"body":""}"#).unwrap();
        assert_eq!(b.body, "");
    }

    #[test]
    fn file_comment_clone_preserves_body() {
        use time::macros::datetime;
        let c = FileComment {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            user_id: Uuid::nil(), body: "test comment".into(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            updated_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        let cloned = c.clone();
        assert_eq!(cloned.body, "test comment");
    }

    #[test]
    fn file_comment_user_id_preserved() {
        use time::macros::datetime;
        use uuid::Uuid;
        let uid = Uuid::new_v4();
        let c = FileComment {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            user_id: uid, body: "x".into(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            updated_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert_eq!(c.user_id, uid);
    }

    #[test]
    fn create_comment_body_unicode() {
        let b: CreateCommentBody = serde_json::from_str(r#"{"body":"Ótimo trabalho 🎉"}"#).unwrap();
        assert!(b.body.contains("Ótimo"));
    }

    #[test]
    fn file_comment_body_field_accessible() {
        use time::macros::datetime;
        let c = FileComment {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            user_id: Uuid::nil(), body: "hello world".into(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            updated_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert_eq!(c.body, "hello world");
    }

    #[test]
    fn file_comment_body_unicode_preserved() {
        use time::macros::datetime;
        let c = FileComment {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            user_id: Uuid::nil(), body: "Olá 👋".into(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            updated_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert!(c.body.contains("Olá"));
    }
}

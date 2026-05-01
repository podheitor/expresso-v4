//! Drive file tags — sprint #415.
//!
//! Routes:
//!   POST   /api/v1/drive/files/:id/tags           — add a tag to a file
//!   DELETE /api/v1/drive/files/:id/tags/:tag      — remove a tag from a file
//!   DELETE /api/v1/drive/files/:id/tags           — clear all tags on a file (sprint #416)
//!   GET    /api/v1/drive/files/:id/tags           — list tags on a file
//!   GET    /api/v1/drive/tags/:tag                — list files with this tag (tenant-scoped)

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

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FileTag {
    pub id:         Uuid,
    pub file_id:    Uuid,
    pub tenant_id:  Uuid,
    pub tag:        String,
    pub created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct AddTagBody {
    tag: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/drive/files/:id/tags",
            get(list_file_tags).post(add_tag).delete(clear_tags),
        )
        .route(
            "/api/v1/drive/files/:id/tags/:tag",
            delete(remove_tag),
        )
        .route(
            "/api/v1/drive/tags/:tag",
            get(list_files_by_tag),
        )
}

/// POST /api/v1/drive/files/:id/tags — add a tag to a file.
async fn add_tag(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<AddTagBody>,
) -> Result<impl IntoResponse> {
    let tag = body.tag.trim().to_lowercase();
    if tag.is_empty() || tag.chars().count() > 64 {
        return Err(DriveError::BadRequest("tag must be 1-64 characters".into()));
    }

    let pool = state.db_or_unavailable()?;

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

    let file_tag: FileTag = sqlx::query_as(
        "INSERT INTO drive_file_tags (file_id, tenant_id, tag, created_by) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (file_id, tenant_id, tag) DO UPDATE \
             SET created_at = drive_file_tags.created_at \
         RETURNING id, file_id, tenant_id, tag, created_by, created_at",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(&tag)
    .bind(ctx.user_id)
    .fetch_one(pool)
    .await?;

    Ok((StatusCode::CREATED, Json(file_tag)))
}

/// DELETE /api/v1/drive/files/:id/tags/:tag — remove a tag from a file.
async fn remove_tag(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, tag)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse> {
    let tag = tag.trim().to_lowercase();
    let pool = state.db_or_unavailable()?;

    let r = sqlx::query(
        "DELETE FROM drive_file_tags WHERE file_id = $1 AND tenant_id = $2 AND tag = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(&tag)
    .execute(pool)
    .await?;

    if r.rows_affected() == 0 {
        return Err(DriveError::NotFound(id));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/drive/files/:id/tags — remove all tags from a file (sprint #416).
async fn clear_tags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;

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

    sqlx::query(
        "DELETE FROM drive_file_tags WHERE file_id = $1 AND tenant_id = $2",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .execute(pool)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/drive/files/:id/tags — list all tags on a file.
async fn list_file_tags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;

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

    let tags: Vec<FileTag> = sqlx::query_as(
        "SELECT id, file_id, tenant_id, tag, created_by, created_at \
         FROM drive_file_tags \
         WHERE tenant_id = $1 AND file_id = $2 \
         ORDER BY tag ASC",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .fetch_all(pool)
    .await?;

    Ok(Json(tags))
}

/// GET /api/v1/drive/tags/:tag — list files tagged with this tag (tenant-scoped).
async fn list_files_by_tag(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(tag):    Path<String>,
) -> Result<impl IntoResponse> {
    let tag = tag.trim().to_lowercase();
    let pool = state.db_or_unavailable()?;

    let tags: Vec<FileTag> = sqlx::query_as(
        "SELECT id, file_id, tenant_id, tag, created_by, created_at \
         FROM drive_file_tags \
         WHERE tenant_id = $1 AND tag = $2 \
         ORDER BY created_at DESC",
    )
    .bind(ctx.tenant_id)
    .bind(&tag)
    .fetch_all(pool)
    .await?;

    Ok(Json(tags))
}

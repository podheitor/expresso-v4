//! Drive file version history — sprint #411 + restore (sprint #420).
//!
//! Routes:
//!   GET  /api/v1/drive/files/:id/versions                  — list versions (newest first)
//!   POST /api/v1/drive/files/:id/versions                  — record a new version snapshot
//!   GET  /api/v1/drive/files/:id/versions/:version         — get a specific version
//!   POST /api/v1/drive/files/:id/versions/:version/restore — copia snapshot de uma versão antiga como nova versão head

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
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
pub struct FileVersion {
    pub id:          Uuid,
    pub file_id:     Uuid,
    pub tenant_id:   Uuid,
    pub version_num: i64,
    pub size_bytes:  i64,
    pub sha256:      Option<String>,
    pub storage_key: Option<String>,
    pub created_by:  Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:  OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct CreateVersionBody {
    size_bytes:  Option<i64>,
    sha256:      Option<String>,
    storage_key: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/drive/files/:id/versions",
            get(list_versions).post(create_version),
        )
        .route(
            "/api/v1/drive/files/:id/versions/:version_num",
            get(get_version),
        )
        .route(
            "/api/v1/drive/files/:id/versions/:version_num/restore",
            post(restore_version),
        )
}

/// POST /api/v1/drive/files/:id/versions/:version_num/restore — restaura o snapshot
/// da versão `version_num` como uma nova versão head, copiando size/sha256/storage_key
/// (sprint #420). Retorna a nova FileVersion criada.
async fn restore_version(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, version_num)): Path<(Uuid, i64)>,
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

    let new_version: Option<FileVersion> = sqlx::query_as(
        "INSERT INTO drive_file_versions \
             (file_id, tenant_id, version_num, size_bytes, sha256, storage_key, created_by) \
         SELECT \
             $1, $2, \
             COALESCE((SELECT MAX(version_num) FROM drive_file_versions WHERE file_id = $1 AND tenant_id = $2), 0) + 1, \
             size_bytes, sha256, storage_key, $4 \
         FROM drive_file_versions \
         WHERE tenant_id = $2 AND file_id = $1 AND version_num = $3 \
         RETURNING id, file_id, tenant_id, version_num, size_bytes, sha256, storage_key, created_by, created_at",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(version_num)
    .bind(ctx.user_id)
    .fetch_optional(pool)
    .await?;

    match new_version {
        Some(v) => Ok((StatusCode::CREATED, Json(v))),
        None    => Err(DriveError::NotFound(id)),
    }
}

/// GET /api/v1/drive/files/:id/versions — list version history (newest first).
async fn list_versions(
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

    let versions: Vec<FileVersion> = sqlx::query_as(
        "SELECT id, file_id, tenant_id, version_num, size_bytes, sha256, storage_key, created_by, created_at \
         FROM drive_file_versions \
         WHERE tenant_id = $1 AND file_id = $2 \
         ORDER BY version_num DESC",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .fetch_all(pool)
    .await?;

    Ok(Json(versions))
}

/// GET /api/v1/drive/files/:id/versions/:version_num — get a specific version.
async fn get_version(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, version_num)): Path<(Uuid, i64)>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;

    let version: Option<FileVersion> = sqlx::query_as(
        "SELECT id, file_id, tenant_id, version_num, size_bytes, sha256, storage_key, created_by, created_at \
         FROM drive_file_versions \
         WHERE tenant_id = $1 AND file_id = $2 AND version_num = $3",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .bind(version_num)
    .fetch_optional(pool)
    .await?;

    match version {
        Some(v) => Ok(Json(v)),
        None    => Err(DriveError::NotFound(id)),
    }
}

/// POST /api/v1/drive/files/:id/versions — record a new version snapshot.
///
/// Assigns the next version_num atomically via MAX(version_num)+1.
async fn create_version(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<CreateVersionBody>,
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

    let version: FileVersion = sqlx::query_as(
        "INSERT INTO drive_file_versions \
             (file_id, tenant_id, version_num, size_bytes, sha256, storage_key, created_by) \
         VALUES ( \
             $1, $2, \
             COALESCE((SELECT MAX(version_num) FROM drive_file_versions WHERE file_id = $1 AND tenant_id = $2), 0) + 1, \
             $3, $4, $5, $6 \
         ) \
         RETURNING id, file_id, tenant_id, version_num, size_bytes, sha256, storage_key, created_by, created_at",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(body.size_bytes.unwrap_or(0))
    .bind(&body.sha256)
    .bind(&body.storage_key)
    .bind(ctx.user_id)
    .fetch_one(pool)
    .await?;

    Ok((StatusCode::CREATED, Json(version)))
}

//! Drive file version history — sprint #411 + restore (sprint #420).
//!
//! Routes:
//!   GET  /api/v1/drive/files/:id/versions                  — list versions (newest first)
//!   POST /api/v1/drive/files/:id/versions                  — record a new version snapshot
//!   GET  /api/v1/drive/files/:id/versions/:version         — get a specific version
//!   POST /api/v1/drive/files/:id/versions/:version/restore — copia snapshot de uma versão antiga como nova versão head
//!   GET  /api/v1/drive/files/:id/versions/:a/diff/:b       — diff metadata entre duas versões (sprint #424)
//!   GET  /api/v1/drive/files/:id/versions-count            — count rápido sem listar (sprint #427)

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
        .route(
            "/api/v1/drive/files/:id/versions/:a/diff/:b",
            get(diff_versions),
        )
        .route(
            "/api/v1/drive/files/:id/versions-count",
            get(count_versions),
        )
}

#[derive(Debug, Serialize)]
struct VersionsCount {
    count: i64,
}

/// GET /api/v1/drive/files/:id/versions-count — retorna apenas a contagem
/// de versões do arquivo (sprint #427), sem listar. Útil pra badges de UI.
/// Path com hífen evita colisão de matching com `versions/:version_num`.
/// 404 se o arquivo não pertence ao tenant ou está soft-deleted.
async fn count_versions(
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

    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM drive_file_versions \
         WHERE tenant_id = $1 AND file_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .fetch_one(pool)
    .await?;

    Ok(Json(VersionsCount { count }))
}

#[derive(Debug, Serialize)]
struct VersionDiff {
    file_id:     Uuid,
    version_a:   i64,
    version_b:   i64,
    size_a:      i64,
    size_b:      i64,
    size_delta:  i64,
    sha_a:       Option<String>,
    sha_b:       Option<String>,
    sha_changed: bool,
}

/// GET /api/v1/drive/files/:id/versions/:a/diff/:b — retorna metadados comparativos
/// entre duas versões do mesmo arquivo (sprint #424). Não computa byte-diff do conteúdo,
/// só `size_delta` (b - a) e `sha_changed`. 404 se qualquer das versões não existe.
async fn diff_versions(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, a, b)): Path<(Uuid, i64, i64)>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i64, i64, Option<String>)> = sqlx::query_as(
        "SELECT version_num, size_bytes, sha256 \
         FROM drive_file_versions \
         WHERE tenant_id = $1 AND file_id = $2 AND version_num = ANY($3::bigint[])",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .bind(vec![a, b])
    .fetch_all(pool)
    .await?;

    let row_a = rows.iter().find(|r| r.0 == a);
    let row_b = rows.iter().find(|r| r.0 == b);
    let (Some(va), Some(vb)) = (row_a, row_b) else {
        return Err(DriveError::NotFound(id));
    };

    let diff = VersionDiff {
        file_id:     id,
        version_a:   a,
        version_b:   b,
        size_a:      va.1,
        size_b:      vb.1,
        size_delta:  vb.1 - va.1,
        sha_a:       va.2.clone(),
        sha_b:       vb.2.clone(),
        sha_changed: va.2 != vb.2,
    };

    Ok(Json(diff))
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

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn file_version_serde_roundtrip() {
        let v = FileVersion {
            id:          Uuid::nil(),
            file_id:     Uuid::nil(),
            tenant_id:   Uuid::nil(),
            version_num: 5,
            size_bytes:  2048,
            sha256:      Some("abc123".into()),
            storage_key: Some("blobs/v5".into()),
            created_by:  Uuid::nil(),
            created_at:  datetime!(2026-05-22 10:00:00 UTC),
        };
        let s = serde_json::to_string(&v).unwrap();
        let back: FileVersion = serde_json::from_str(&s).unwrap();
        assert_eq!(back.version_num, 5);
        assert_eq!(back.size_bytes, 2048);
    }

    #[test]
    fn create_version_body_all_optional() {
        let json = r#"{}"#;
        let b: CreateVersionBody = serde_json::from_str(json).unwrap();
        assert!(b.size_bytes.is_none());
        assert!(b.sha256.is_none());
        assert!(b.storage_key.is_none());
    }

    #[test]
    fn create_version_body_full() {
        let json = r#"{"size_bytes":4096,"sha256":"deadbeef","storage_key":"blobs/new"}"#;
        let b: CreateVersionBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.size_bytes, Some(4096));
        assert_eq!(b.sha256.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn create_version_body_roundtrip_storage_key_optional() {
        let b: CreateVersionBody = serde_json::from_str(r#"{"storage_key":"blobs/v2"}"#).unwrap();
        assert_eq!(b.storage_key.as_deref(), Some("blobs/v2"));
        assert!(b.size_bytes.is_none());
    }

    #[test]
    fn create_version_body_size_zero_allowed() {
        let b: CreateVersionBody = serde_json::from_str(r#"{"size_bytes":0}"#).unwrap();
        assert_eq!(b.size_bytes, Some(0));
    }

    #[test]
    fn create_version_body_all_absent() {
        let b: CreateVersionBody = serde_json::from_str(r#"{}"#).unwrap();
        assert!(b.size_bytes.is_none());
        assert!(b.sha256.is_none());
        assert!(b.storage_key.is_none());
    }

    #[test]
    fn create_version_body_sha256_optional() {
        let b: CreateVersionBody = serde_json::from_str(r#"{"size_bytes":100}"#).unwrap();
        assert_eq!(b.size_bytes, Some(100));
        assert!(b.sha256.is_none());
    }

    #[test]
    fn create_version_body_storage_key_optional() {
        let b: CreateVersionBody = serde_json::from_str(r#"{"storage_key":"blobs/abc123"}"#).unwrap();
        assert_eq!(b.storage_key.as_deref(), Some("blobs/abc123"));
        assert!(b.size_bytes.is_none());
    }

    #[test]
    fn create_version_body_all_none_when_empty() {
        let b: CreateVersionBody = serde_json::from_str(r#"{}"#).unwrap();
        assert!(b.size_bytes.is_none());
        assert!(b.sha256.is_none());
    }

    #[test]
    fn create_version_body_sha256_preserved() {
        let b: CreateVersionBody = serde_json::from_str(r#"{"sha256":"abc123","size_bytes":1024}"#).unwrap();
        assert_eq!(b.sha256.as_deref(), Some("abc123"));
        assert_eq!(b.size_bytes, Some(1024));
    }

    #[test]
    fn create_version_body_storage_key_preserved() {
        let b: CreateVersionBody = serde_json::from_str(r#"{"storage_key":"blobs/abc123"}"#).unwrap();
        assert_eq!(b.storage_key.as_deref(), Some("blobs/abc123"));
    }

    #[test]
    fn create_version_body_sha256_short_value_preserved() {
        let b: CreateVersionBody = serde_json::from_str(r#"{"sha256":"abc123"}"#).unwrap();
        assert_eq!(b.sha256.as_deref(), Some("abc123"));
    }

    #[test]
    fn create_version_body_all_none_on_empty_object() {
        let b: CreateVersionBody = serde_json::from_str(r#"{}"#).unwrap();
        assert!(b.size_bytes.is_none() && b.sha256.is_none() && b.storage_key.is_none());
    }

    #[test]
    fn create_version_body_size_bytes_preserved() {
        let b: CreateVersionBody = serde_json::from_str(r#"{"size_bytes":512}"#).unwrap();
        assert_eq!(b.size_bytes, Some(512));
    }

    #[test]
    fn create_version_body_all_fields_set() {
        let b: CreateVersionBody = serde_json::from_str(
            r#"{"size_bytes":2048,"sha256":"deadbeef","storage_key":"blobs/v1"}"#,
        ).unwrap();
        assert_eq!(b.size_bytes, Some(2048));
        assert_eq!(b.sha256.as_deref(), Some("deadbeef"));
        assert_eq!(b.storage_key.as_deref(), Some("blobs/v1"));
    }

    #[test]
    fn file_version_sha256_optional_none() {
        use time::macros::datetime;
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_num: 1, size_bytes: 0, sha256: None, storage_key: None,
            created_by: Uuid::nil(), created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert!(v.sha256.is_none());
    }

    #[test]
    fn file_version_storage_key_none_serializes_null() {
        use time::macros::datetime;
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_num: 2, size_bytes: 512, sha256: None, storage_key: None,
            created_by: Uuid::nil(), created_at: datetime!(2026-03-01 00:00:00 UTC),
        };
        let j: serde_json::Value = serde_json::to_value(&v).unwrap();
        assert!(j["storage_key"].is_null());
    }

    #[test]
    fn version_diff_size_delta_computed() {
        let size_a: i64 = 1024;
        let size_b: i64 = 2048;
        let delta = size_b - size_a;
        assert_eq!(delta, 1024);
    }

    #[test]
    fn create_version_body_size_large_value_preserved() {
        let json = r#"{"size_bytes":9007199254740992}"#;
        let b: CreateVersionBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.size_bytes, Some(9_007_199_254_740_992));
    }
}

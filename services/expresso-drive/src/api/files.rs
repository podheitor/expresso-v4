//! Drive files API — list, upload (w/ auto-versioning), download, delete, mkdir, trash, versions.

use axum::{
    body::Bytes,
    extract::{Multipart, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, head, patch, post},
    Json, Router,
};
use time::OffsetDateTime;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use crate::{
    api::context::RequestCtx,
    domain::{DriveFile, FileRepo, FileVersion, NewFile, NewVersion, QuotaRepo, UserUsage, VersionRepo},
    error::{DriveError, Result},
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/drive/files",                       get(list).post(upload))
        .route("/api/v1/drive/files/mkdir",                 post(mkdir))
        .route("/api/v1/drive/files/:id",                   get(download).delete(delete).head(head_file))
        .route("/api/v1/drive/files/:id/preview",           get(preview))
        .route("/api/v1/drive/files/:id/metadata",          get(metadata).patch(rename))
        .route("/api/v1/drive/files/search",                 get(search))
        .route("/api/v1/drive/files/bulk-trash",             post(bulk_trash))
        .route("/api/v1/drive/files/bulk-move",              post(bulk_move))
        .route("/api/v1/drive/files/:id/copy",               post(copy_file))
        .route("/api/v1/drive/files/:id/move",              post(move_file))
        .route("/api/v1/drive/files/:id/restore",           post(restore))
        .route("/api/v1/drive/files/:id/versions",          get(list_versions))
        .route("/api/v1/drive/files/:id/versions/:v",       get(download_version))
        .route("/api/v1/drive/trash",                       get(trash))
        .route("/api/v1/drive/quota",                       get(quota))
        .route("/api/v1/drive/users/:user_id/usage",        get(user_usage))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub parent_id: Option<Uuid>,
    /// Filter by kind: "file" or "folder". Omit for both.
    pub kind:      Option<String>,
    /// Sort column: name | updated_at | created_at | size_bytes. Default: name.
    pub sort:      Option<String>,
    /// Sort direction: asc | desc. Default: asc.
    pub order:     Option<String>,
    /// Max rows to return (1–500, default 200).
    pub limit:     Option<i64>,
    /// Rows to skip (default 0).
    pub offset:    Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct MkdirBody {
    pub name:      String,
    pub parent_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteQuery {
    #[serde(default)]
    pub permanent: bool,
}

async fn list(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<ListQuery>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let max_updated: Option<OffsetDateTime> = if let Some(pid) = q.parent_id {
        sqlx::query_scalar(
            "SELECT MAX(updated_at) FROM drive_files WHERE tenant_id = $1 AND parent_id = $2 AND deleted_at IS NULL",
        )
        .bind(ctx.tenant_id)
        .bind(pid)
        .fetch_one(pool)
        .await
        .unwrap_or(None)
    } else {
        sqlx::query_scalar(
            "SELECT MAX(updated_at) FROM drive_files WHERE tenant_id = $1 AND parent_id IS NULL AND deleted_at IS NULL",
        )
        .bind(ctx.tenant_id)
        .fetch_one(pool)
        .await
        .unwrap_or(None)
    };
    if let Some(ts) = max_updated {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }
    if let Some(ref k) = q.kind {
        if k != "file" && k != "folder" {
            return Err(DriveError::BadRequest("kind must be 'file' or 'folder'".into()));
        }
    }
    let sort  = q.sort.as_deref().unwrap_or("name");
    let order = q.order.as_deref().unwrap_or("asc");
    if !matches!(sort, "name" | "updated_at" | "created_at" | "size_bytes") {
        return Err(DriveError::BadRequest(
            "sort must be one of: name, updated_at, created_at, size_bytes".into()
        ));
    }
    if !matches!(order, "asc" | "desc") {
        return Err(DriveError::BadRequest("order must be 'asc' or 'desc'".into()));
    }
    let limit  = q.limit.unwrap_or(200).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let mut rows = FileRepo::new(pool)
        .list_children_paged(ctx.tenant_id, q.parent_id, sort, order, limit, offset)
        .await?;
    if let Some(k) = q.kind.as_deref() {
        rows.retain(|f| f.kind == k);
    }
    let mut resp = Json(rows).into_response();
    if let Some(ts) = max_updated {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

async fn mkdir(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<MkdirBody>,
) -> Result<(StatusCode, Json<DriveFile>)> {
    let pool = state.db_or_unavailable()?;
    let name = sanitize_name(&body.name)?;
    let row  = FileRepo::new(pool).insert(&NewFile {
        tenant_id:     ctx.tenant_id,
        owner_user_id: ctx.user_id,
        parent_id:     body.parent_id,
        name,
        kind:          "folder".into(),
        mime_type:     None,
        size_bytes:    0,
        sha256:        None,
        storage_key:   None,
    }).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn upload(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    mut mp:       Multipart,
) -> Result<(StatusCode, Json<DriveFile>)> {
    let pool = state.db_or_unavailable()?;

    let mut parent_id: Option<Uuid>    = None;
    let mut name:      Option<String>  = None;
    let mut mime:      Option<String>  = None;
    let mut data:      Option<Bytes>   = None;

    while let Some(field) = mp.next_field().await.map_err(|e| DriveError::BadRequest(e.to_string()))? {
        match field.name().unwrap_or("") {
            "parent_id" => {
                let v = field.text().await.map_err(|e| DriveError::BadRequest(e.to_string()))?;
                if !v.trim().is_empty() {
                    parent_id = Some(Uuid::parse_str(v.trim())
                        .map_err(|_| DriveError::BadRequest("invalid parent_id".into()))?);
                }
            }
            "file" => {
                name = field.file_name().map(|s| s.to_string());
                mime = field.content_type().map(|s| s.to_string());
                data = Some(field.bytes().await.map_err(|e| DriveError::BadRequest(e.to_string()))?);
            }
            _ => {}
        }
    }

    let bytes = data.ok_or(DriveError::BadRequest("missing file part".into()))?;
    let fname = sanitize_name(&name.unwrap_or_default())?;

    // Quota enforcement — rejeita antes de tocar o disco.
    let quota = QuotaRepo::new(pool).get(ctx.tenant_id).await?;
    if !quota.fits(bytes.len() as i64) {
        return Err(DriveError::QuotaExceeded);
    }


    // Hash + persist blob. storage_key = random UUID → evita colisão
    // cross-tenant e mantém layout on-disk flat.
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let key = Uuid::new_v4().to_string();
    let root = state.data_root();
    fs::create_dir_all(root).await?;
    let path: PathBuf = root.join(&key);
    let mut f = fs::File::create(&path).await?;
    f.write_all(&bytes).await?;
    f.flush().await?;

    let repo     = FileRepo::new(pool);
    let ver_repo = VersionRepo::new(pool);

    // Existing sibling w/ same name → archive current → overwrite row.
    if let Some(existing) = repo.find_by_name(ctx.tenant_id, parent_id, &fname).await? {
        if existing.kind != "file" {
            // Folder collision → cleanup new blob + conflict.
            let _ = fs::remove_file(&path).await;
            return Err(DriveError::Conflict("a folder with this name already exists".into()));
        }

        // Archive previous content (if any) before overwrite.
        if let Some(prev_key) = existing.storage_key.as_deref() {
            let next_no = ver_repo.next_no(ctx.tenant_id, existing.id).await?;
            ver_repo.insert(&NewVersion {
                file_id:     existing.id,
                tenant_id:   ctx.tenant_id,
                version_no:  next_no,
                storage_key: prev_key,
                size_bytes:  existing.size_bytes,
                sha256:      existing.sha256.as_deref(),
                mime_type:   existing.mime_type.as_deref(),
                created_by:  existing.owner_user_id,
            }).await?;
        }

        let updated = repo.update_content(
            ctx.tenant_id, existing.id,
            &key, bytes.len() as i64,
            Some(&sha), mime.as_deref(),
        ).await;

        if updated.is_err() {
            let _ = fs::remove_file(&path).await;
        }
        let updated = updated?;
        tracing::info!(target: "audit",
            event = "drive.file.version",
            tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %updated.id);
        return Ok((StatusCode::OK, Json(updated)));
    }

    // New file → plain insert.
    let row = repo.insert(&NewFile {
        tenant_id:     ctx.tenant_id,
        owner_user_id: ctx.user_id,
        parent_id,
        name:          fname,
        kind:          "file".into(),
        mime_type:     mime,
        size_bytes:    bytes.len() as i64,
        sha256:        Some(sha),
        storage_key:   Some(key.clone()),
    }).await;

    if row.is_err() {
        let _ = fs::remove_file(&path).await;
    }
    let row = row?;
    tracing::info!(target: "audit",
        event = "drive.file.upload",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %row.id);
    Ok((StatusCode::CREATED, Json(row)))
}

async fn metadata(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let f = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    let etag = format!("\"{}-{}\"", f.updated_at.unix_timestamp(), f.id);
    let lm = f.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if f.updated_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    let mut resp = Json(f).into_response();
    resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(resp)
}

async fn head_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let f = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    let etag = format!("\"{}-{}\"", f.updated_at.unix_timestamp(), f.id);
    let lm = f.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if f.updated_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    let mut resp = StatusCode::OK.into_response();
    resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    if let Ok(size) = HeaderValue::from_str(&f.size_bytes.to_string()) {
        resp.headers_mut().insert(header::CONTENT_LENGTH, size);
    }
    Ok(resp)
}

#[derive(Debug, Deserialize)]
struct RenameBody {
    name: String,
}

#[derive(Debug, Deserialize)]
struct MoveBody {
    /// Destination folder id; omit or null to move to root.
    parent_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct BulkTrashBody {
    /// File/folder ids to soft-delete (max 200).
    ids: Vec<Uuid>,
}

/// POST /api/v1/drive/files/bulk-trash — soft-delete up to 200 items atomically.
async fn bulk_trash(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<BulkTrashBody>,
) -> Result<Json<serde_json::Value>> {
    if body.ids.is_empty() {
        return Err(DriveError::BadRequest("ids must not be empty".into()));
    }
    if body.ids.len() > 200 {
        return Err(DriveError::BadRequest(format!(
            "too many ids: {} (max 200)", body.ids.len()
        )));
    }
    let pool    = state.db_or_unavailable()?;
    let trashed = FileRepo::new(pool).bulk_trash(ctx.tenant_id, &body.ids).await?;
    tracing::info!(target: "audit",
        event = "drive.file.bulk_trash",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, count = trashed);
    Ok(Json(serde_json::json!({ "trashed": trashed })))
}

#[derive(Debug, Deserialize)]
struct BulkMoveBody {
    /// File/folder ids to move (max 200).
    ids:       Vec<Uuid>,
    /// Destination folder id; omit or null to move to root.
    parent_id: Option<Uuid>,
}

/// POST /api/v1/drive/files/bulk-move — atomically move up to 200 items.
async fn bulk_move(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<BulkMoveBody>,
) -> Result<Json<Vec<DriveFile>>> {
    if body.ids.is_empty() {
        return Err(DriveError::BadRequest("ids must not be empty".into()));
    }
    if body.ids.len() > 200 {
        return Err(DriveError::BadRequest(format!(
            "too many ids: {} (max 200)", body.ids.len()
        )));
    }
    let pool = state.db_or_unavailable()?;
    if let Some(parent) = body.parent_id {
        let target = FileRepo::new(pool).get(ctx.tenant_id, parent).await?;
        if target.kind != "folder" {
            return Err(DriveError::BadRequest("parent_id must be a folder".into()));
        }
        // Prevent moving any of the selected items into themselves.
        if body.ids.contains(&target.id) {
            return Err(DriveError::BadRequest("cannot move a folder into itself".into()));
        }
    }
    let rows = FileRepo::new(pool).bulk_move(ctx.tenant_id, &body.ids, body.parent_id).await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
struct CopyBody {
    /// Optional destination name. Defaults to "<original name> (cópia)".
    name:      Option<String>,
    /// Destination parent; omit or null to place at root.
    parent_id: Option<Uuid>,
}

/// POST /api/v1/drive/files/:id/copy — shallow copy: new row, same blob.
async fn copy_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<CopyBody>,
) -> Result<(StatusCode, Json<DriveFile>)> {
    let pool = state.db_or_unavailable()?;
    let src  = FileRepo::new(pool).get(ctx.tenant_id, id).await?;

    let raw_name = body.name.unwrap_or_else(|| format!("{} (cópia)", src.name));
    let new_name = sanitize_name(&raw_name)?;

    if let Some(parent) = body.parent_id {
        let target = FileRepo::new(pool).get(ctx.tenant_id, parent).await?;
        if target.kind != "folder" {
            return Err(DriveError::BadRequest("parent_id must be a folder".into()));
        }
    }

    let row = FileRepo::new(pool)
        .copy_file(ctx.tenant_id, ctx.user_id, id, new_name, body.parent_id)
        .await?;
    tracing::info!(target: "audit",
        event = "drive.file.copy",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        src_id = %id, copy_id = %row.id);
    Ok((StatusCode::CREATED, Json(row)))
}

/// POST /api/v1/drive/files/:id/move — move a file or folder to a different parent.
async fn move_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<MoveBody>,
) -> Result<Json<DriveFile>> {
    if let Some(parent) = body.parent_id {
        // Sanity: target folder must exist in the same tenant.
        let pool = state.db_or_unavailable()?;
        let target = FileRepo::new(pool).get(ctx.tenant_id, parent).await?;
        if target.kind != "folder" {
            return Err(DriveError::BadRequest("parent_id must be a folder".into()));
        }
        if target.id == id {
            return Err(DriveError::BadRequest("cannot move a folder into itself".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let f = FileRepo::new(pool).move_to(ctx.tenant_id, id, body.parent_id).await?;
    Ok(Json(f))
}

/// PATCH /api/v1/drive/files/:id/metadata — rename a file or folder.
async fn rename(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<RenameBody>,
) -> Result<Json<DriveFile>> {
    let name = sanitize_name(&body.name)?;
    let pool = state.db_or_unavailable()?;
    let f = FileRepo::new(pool).rename(ctx.tenant_id, id, name).await?;
    Ok(Json(f))
}

async fn download(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let f    = FileRepo::new(pool).get(ctx.tenant_id, id).await?;

    if f.kind != "file" {
        return Err(DriveError::BadRequest("target is a folder".into()));
    }
    let key = f.storage_key.as_deref()
        .ok_or_else(|| DriveError::BadRequest("file has no content".into()))?;
    let bytes = fs::read(state.data_root().join(key)).await?;
    tracing::info!(target: "audit",
        event = "drive.file.download",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %id);
    Ok(attachment_response(&f.name, f.mime_type.as_deref(), bytes))
}

/// GET /api/v1/drive/files/:id/preview
///
/// Returns the file content with `Content-Disposition: inline` so browsers
/// render it directly rather than downloading it. Supported for image/* and
/// application/pdf; returns 415 Unsupported Media Type for all other MIME types.
async fn preview(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let f    = FileRepo::new(pool).get(ctx.tenant_id, id).await?;

    if f.kind != "file" {
        return Err(DriveError::BadRequest("target is a folder".into()));
    }

    let mime = f.mime_type.as_deref().unwrap_or("application/octet-stream");
    let previewable = mime.starts_with("image/") || mime == "application/pdf";
    if !previewable {
        return Ok((StatusCode::UNSUPPORTED_MEDIA_TYPE,
            axum::Json(serde_json::json!({"error": "preview not supported for this file type", "mime": mime})))
            .into_response());
    }

    let key   = f.storage_key.as_deref()
        .ok_or_else(|| DriveError::BadRequest("file has no content".into()))?;
    let bytes = fs::read(state.data_root().join(key)).await?;

    let ct: HeaderValue = mime.parse()
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));

    let ascii: String = f.name.chars().map(|c| {
        if c.is_ascii_graphic() && c != '"' && c != '\\' { c } else { '_' }
    }).collect();
    let ascii = if ascii.is_empty() { "preview".to_string() } else { ascii };
    let cd: HeaderValue = format!("inline; filename=\"{ascii}\"")
        .parse()
        .unwrap_or_else(|_| HeaderValue::from_static("inline"));

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, ct);
    headers.insert(header::CONTENT_DISPOSITION, cd);

    Ok((StatusCode::OK, headers, bytes).into_response())
}

async fn delete(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Query(q):     Query<DeleteQuery>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = FileRepo::new(pool);
    if q.permanent {
        let key = repo.purge(ctx.tenant_id, id).await?;
        let Some(key) = key else { return Err(DriveError::NotFound(id)); };
        if !key.is_empty() {
            let path = state.data_root().join(&key);
            if let Err(e) = fs::remove_file(&path).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(target: "audit",
                        event = "drive.purge.blob_unlink_failed",
                        file_id = %id, error = %e);
                }
            }
        }
        tracing::info!(target: "audit",
            event = "drive.file.purge",
            tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %id);
        return Ok(StatusCode::NO_CONTENT);
    }
    let removed = repo.soft_delete(ctx.tenant_id, id).await?;
    if removed == 0 { return Err(DriveError::NotFound(id)); }
    tracing::info!(target: "audit",
        event = "drive.file.trash",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %id);
    Ok(StatusCode::NO_CONTENT)
}

async fn restore(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<DriveFile>> {
    let pool = state.db_or_unavailable()?;
    let row  = FileRepo::new(pool).restore(ctx.tenant_id, id).await?;
    tracing::info!(target: "audit",
        event = "drive.file.restore",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %id);
    Ok(Json(row))
}

async fn trash(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let max_updated: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(updated_at) FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NOT NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);
    if let Some(ts) = max_updated {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }
    let rows = FileRepo::new(pool).list_trash(ctx.tenant_id).await?;
    let mut resp = Json(rows).into_response();
    if let Some(ts) = max_updated {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

async fn list_versions(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let f = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    let max_created: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(created_at) FROM file_versions WHERE tenant_id = $1 AND file_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);
    let max_ts = max_created.unwrap_or(f.updated_at);
    let lm = max_ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if max_ts <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    let rows = VersionRepo::new(pool).list(ctx.tenant_id, id).await?;
    let mut resp = Json(rows).into_response();
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(resp)
}

async fn download_version(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, v)): Path<(Uuid, i32)>,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    // Tenant-gate.
    let parent = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    let ver = VersionRepo::new(pool).get(ctx.tenant_id, id, v).await?
        .ok_or(DriveError::NotFound(id))?;
    let bytes = fs::read(state.data_root().join(&ver.storage_key)).await?;
    tracing::info!(target: "audit",
        event = "drive.file.download_version",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        file_id = %id, version_no = v);
    let filename = format!("{}.v{}", parent.name, v);
    Ok(attachment_response(&filename, ver.mime_type.as_deref(), bytes))
}

pub(crate) fn attachment_response(name: &str, mime: Option<&str>, bytes: Vec<u8>) -> Response {
    let mut headers = HeaderMap::new();
    let ct: axum::http::HeaderValue = mime
        .and_then(|m| m.parse().ok())
        .unwrap_or_else(|| axum::http::HeaderValue::from_static("application/octet-stream"));
    headers.insert(header::CONTENT_TYPE, ct);

    // Build a safe `filename=` parameter: ASCII printable only, no quotes,
    // backslashes or control chars. Non-conforming bytes become `_`. Also
    // emit the RFC 5987 `filename*` form so clients can recover the
    // original UTF-8 name. Both are header-injection-safe by construction.
    let cd = build_content_disposition(name);
    let cd_val = axum::http::HeaderValue::from_str(&cd)
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static("attachment"));
    headers.insert(header::CONTENT_DISPOSITION, cd_val);

    (StatusCode::OK, headers, bytes).into_response()
}

fn build_content_disposition(name: &str) -> String {
    let ascii: String = name.chars().map(|c| {
        if c.is_ascii_graphic() && c != '"' && c != '\\' { c } else { '_' }
    }).collect();
    let ascii = if ascii.is_empty() { "download".to_string() } else { ascii };
    let pct = percent_encode_filename(name);
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{pct}")
}

/// RFC 5987 percent-encoding for the `filename*` parameter. Encodes every
/// byte that is not in `attr-char` per RFC 5987 §3.2.1.
fn percent_encode_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len() * 3);
    for b in name.as_bytes() {
        let c = *b;
        let attr_char = c.is_ascii_alphanumeric()
            || matches!(c, b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~');
        if attr_char {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

fn sanitize_name(raw: &str) -> Result<String> {
    let s = raw.trim();
    if s.is_empty() || s.contains('/') || s.contains('\\') || s == "." || s == ".." {
        return Err(DriveError::BadRequest("invalid name".into()));
    }
    // Reject ASCII control chars (incl. CR, LF, NUL, TAB). Non-ASCII printables
    // are fine — `build_content_disposition` handles encoding for headers.
    if s.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}') {
        return Err(DriveError::BadRequest("name has control characters".into()));
    }
    if s.len() > 255 {
        return Err(DriveError::BadRequest("name too long".into()));
    }
    Ok(s.to_string())
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q:      String,
    #[serde(default = "default_search_limit")]
    limit:  i64,
    #[serde(default)]
    offset: i64,
}
fn default_search_limit() -> i64 { 50 }

/// GET /api/v1/drive/files/search?q=<term>&limit=50&offset=0
/// Case-insensitive substring match on name within the caller's tenant.
async fn search(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<SearchQuery>,
) -> Result<Json<Vec<DriveFile>>> {
    if q.q.trim().is_empty() {
        return Err(DriveError::BadRequest("q must not be empty".into()));
    }
    let limit  = q.limit.clamp(1, 200);
    let offset = q.offset.max(0);
    let pool   = state.db_or_unavailable()?;
    let pattern = format!("%{}%", q.q.replace('%', "\\%").replace('_', "\\_"));
    let rows: Vec<DriveFile> = sqlx::query_as(
        "SELECT id, tenant_id, owner_user_id, parent_id, name, kind, \
                mime_type, size_bytes, sha256, storage_key, created_at, updated_at, deleted_at \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND name ILIKE $2 ESCAPE '\\' \
         ORDER BY lower(name) \
         LIMIT $3 OFFSET $4",
    )
    .bind(ctx.tenant_id)
    .bind(&pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(Json(rows))
}

async fn quota(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;

    let max_ts: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(updated_at) FROM drive_files WHERE tenant_id = $1 AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);

    if let Some(ts) = max_ts {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

    let q = QuotaRepo::new(pool).get(ctx.tenant_id).await?;
    let mut resp = Json(q).into_response();
    if let Some(ts) = max_ts {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

/// GET /api/v1/drive/users/:user_id/usage — bytes owned by a user in this tenant.
async fn user_usage(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(user_id): Path<Uuid>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;

    let max_ts: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(updated_at) FROM drive_files \
         WHERE tenant_id = $1 AND owner_user_id = $2 AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);

    if let Some(ts) = max_ts {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

    let usage = QuotaRepo::new(pool).get_user_usage(ctx.tenant_id, user_id).await?;
    let mut resp = Json(usage).into_response();
    if let Some(ts) = max_ts {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

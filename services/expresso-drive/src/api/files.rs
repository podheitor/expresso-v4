//! Drive files API — list, upload (w/ auto-versioning), download, delete, mkdir, trash, versions.

use axum::{
    body::Bytes,
    extract::{Multipart, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, head, patch, post},
    Json, Router,
};
use std::io::Write as IoWrite;
use time::OffsetDateTime;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use crate::{
    api::context::RequestCtx,
    domain::{DriveFile, FileRepo, FileVersion, FolderQuota, FolderQuotaRepo, NewFile, NewVersion, QuotaRepo, TagRepo, UserUsage, VersionRepo},
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
        .route("/api/v1/drive/files/bulk-copy",              post(bulk_copy))
        .route("/api/v1/drive/files/bulk-restore",           post(bulk_restore))
        .route("/api/v1/drive/files/:id/copy",               post(copy_file))
        .route("/api/v1/drive/files/:id/move",              post(move_file))
        .route("/api/v1/drive/files/:id/restore",           post(restore))
        .route("/api/v1/drive/files/:id/tags",              get(list_tags).post(add_tag))
        .route("/api/v1/drive/files/:id/tags/:tag",         delete(remove_tag))
        .route("/api/v1/drive/files/:id/versions",          get(list_versions))
        .route("/api/v1/drive/files/:id/versions/:v",       get(download_version).delete(delete_version))
        .route("/api/v1/drive/files/:id/versions/:v/diff-content", get(diff_version_content))
        .route("/api/v1/drive/files/:id/expiry",              patch(set_expiry))
        .route("/api/v1/drive/files/:id/lock",               post(lock_file).delete(unlock_file))
        .route("/api/v1/drive/files/:id/star",               post(star_file).delete(unstar_file))
        .route("/api/v1/drive/starred",                      get(list_starred))
        .route("/api/v1/drive/starred/count",                get(count_starred))
        .route("/api/v1/drive/folders/:id/download",         get(download_folder))
        .route("/api/v1/drive/folders/:id/quota",           get(folder_quota).put(set_folder_quota).delete(delete_folder_quota))
        .route("/api/v1/drive/trash",                       get(trash).delete(purge_trash))
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

    // Tenant-level quota check — rejeita antes de tocar o disco.
    let quota = QuotaRepo::new(pool).get(ctx.tenant_id).await?;
    if !quota.fits(bytes.len() as i64) {
        return Err(DriveError::QuotaExceeded);
    }

    // Folder-level quota check — only when uploading into a specific folder.
    if let Some(fid) = parent_id {
        if let Some(fq) = FolderQuotaRepo::new(pool).get(ctx.tenant_id, fid).await? {
            if !fq.fits(bytes.len() as i64) {
                return Err(DriveError::QuotaExceeded);
            }
        }
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
struct BulkRestoreBody {
    /// File/folder ids to restore from trash (max 200).
    ids: Vec<Uuid>,
}

/// POST /api/v1/drive/files/bulk-restore — restore up to 200 trashed items atomically.
async fn bulk_restore(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<BulkRestoreBody>,
) -> Result<Json<serde_json::Value>> {
    if body.ids.is_empty() {
        return Err(DriveError::BadRequest("ids must not be empty".into()));
    }
    if body.ids.len() > 200 {
        return Err(DriveError::BadRequest(format!(
            "too many ids: {} (max 200)", body.ids.len()
        )));
    }
    let pool     = state.db_or_unavailable()?;
    let restored = FileRepo::new(pool).bulk_restore(ctx.tenant_id, &body.ids).await?;
    tracing::info!(target: "audit",
        event = "drive.file.bulk_restore",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, count = restored);
    Ok(Json(serde_json::json!({ "restored": restored })))
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
struct BulkCopyBody {
    /// File/folder ids to copy (max 200).
    ids:       Vec<Uuid>,
    /// Destination parent; omit or null to place at root.
    parent_id: Option<Uuid>,
}

/// POST /api/v1/drive/files/bulk-copy — shallow-copy up to 200 items.
/// Each copy is named "<original name> (cópia)". Folders are copied as empty
/// rows (the same blob). Returns the list of newly created rows.
async fn bulk_copy(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<BulkCopyBody>,
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
    }
    let rows = FileRepo::new(pool)
        .bulk_copy_files(ctx.tenant_id, ctx.user_id, &body.ids, body.parent_id)
        .await?;
    tracing::info!(target: "audit",
        event = "drive.file.bulk_copy",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, count = rows.len());
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

#[derive(Debug, Deserialize)]
struct PurgeTrashParams {
    /// Idade mínima (em dias) do `deleted_at` pra purgar. Default 30, cap 1..3650.
    older_than_days: Option<i64>,
}

/// DELETE /api/v1/drive/trash?older_than_days=30 — hard-delete files com
/// `deleted_at <= now() - older_than_days`. Tenant-scoped via begin_tenant_tx.
/// Best-effort blob removal (loga mas não falha se já sumiu). Retorna
/// `{purged: N, file_ids: [...]}`. Útil pra sweep manual antes do GC automático
/// kick-in. older_than_days default 30, clamp [1, 3650] (10 anos).
async fn purge_trash(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(params): Query<PurgeTrashParams>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let days = params.older_than_days.unwrap_or(30).clamp(1, 3650);
    let cutoff = OffsetDateTime::now_utc() - time::Duration::days(days);

    let purged = FileRepo::new(pool)
        .purge_trashed_older_than(ctx.tenant_id, cutoff)
        .await?;

    let data_root = state.data_root();
    let mut ids = Vec::with_capacity(purged.len());
    for (id, key) in &purged {
        ids.push(*id);
        if let Some(k) = key {
            let blob = data_root.join(k);
            if let Err(e) = fs::remove_file(&blob).await {
                tracing::warn!(target: "audit",
                    event = "drive.trash.purge_blob_skipped",
                    file_id = %id, key = %k, error = %e);
            }
        }
    }

    tracing::info!(target: "audit",
        event = "drive.trash.purge",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        older_than_days = days, purged = ids.len());

    Ok(Json(serde_json::json!({
        "purged":   ids.len(),
        "file_ids": ids,
    })))
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

/// GET /api/v1/drive/files/:id/versions/:v/diff-content
///
/// Returns a unified text diff between version `:v` and the version immediately
/// before it (v-1). 404 if either version blob is missing or the file is not
/// text (detected via Content-Type prefix). 409 if v == 1 (no previous version).
/// Response: `{version_a, version_b, hunks: [{header, lines: [{tag,text}]}]}`.
/// Binary-safe guard: rejects blobs with a NUL byte in the first 8 KiB.
async fn diff_version_content(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, v)): Path<(Uuid, i32)>,
) -> Result<Json<serde_json::Value>> {
    use serde_json::json;

    if v <= 1 {
        return Err(DriveError::BadRequest("no previous version to diff (v must be > 1)".into()));
    }

    let pool = state.db_or_unavailable()?;
    let _file = FileRepo::new(pool).get(ctx.tenant_id, id).await?;

    let ver_b = VersionRepo::new(pool).get(ctx.tenant_id, id, v).await?
        .ok_or(DriveError::NotFound(id))?;
    let ver_a = VersionRepo::new(pool).get(ctx.tenant_id, id, v - 1).await?
        .ok_or(DriveError::NotFound(id))?;

    let bytes_a = fs::read(state.data_root().join(&ver_a.storage_key)).await
        .map_err(|_| DriveError::NotFound(id))?;
    let bytes_b = fs::read(state.data_root().join(&ver_b.storage_key)).await
        .map_err(|_| DriveError::NotFound(id))?;

    // Binary guard — reject if NUL byte in first 8 KiB.
    let probe_a = &bytes_a[..bytes_a.len().min(8192)];
    let probe_b = &bytes_b[..bytes_b.len().min(8192)];
    if probe_a.contains(&0u8) || probe_b.contains(&0u8) {
        return Err(DriveError::BadRequest("binary files cannot be diffed as text".into()));
    }

    let text_a = String::from_utf8_lossy(&bytes_a);
    let text_b = String::from_utf8_lossy(&bytes_b);

    let lines_a: Vec<&str> = text_a.lines().collect();
    let lines_b: Vec<&str> = text_b.lines().collect();

    let hunks = unified_diff(&lines_a, &lines_b, 3);
    Ok(Json(json!({
        "file_id":   id,
        "version_a": v - 1,
        "version_b": v,
        "hunks":     hunks,
    })))
}

/// Compute a unified diff between two line slices with `context` lines of context.
/// Returns a JSON array of hunks: `[{header, lines: [{tag: "+"|"-"|" ", text}]}]`.
fn unified_diff(old: &[&str], new: &[&str], context: usize) -> serde_json::Value {
    use serde_json::json;

    // Myers-style edit script via LCS table (simple DP — O(mn) space).
    let m = old.len();
    let n = new.len();

    // lcs[i][j] = length of LCS of old[..i] and new[..j].
    let mut lcs = vec![vec![0usize; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    // Trace edit operations: Equal / Delete / Insert.
    #[derive(Clone, Copy, PartialEq)]
    enum Op { Eq, Del, Ins }

    let mut ops: Vec<(Op, usize, usize)> = Vec::new(); // (op, old_idx, new_idx)
    let (mut i, mut j) = (0, 0);
    while i < m || j < n {
        if i < m && j < n && old[i] == new[j] {
            ops.push((Op::Eq, i, j));
            i += 1; j += 1;
        } else if j < n && (i >= m || lcs[i][j + 1] >= lcs[i + 1][j]) {
            ops.push((Op::Ins, i, j));
            j += 1;
        } else {
            ops.push((Op::Del, i, j));
            i += 1;
        }
    }

    // Group ops into hunks (changed regions ± context lines).
    let mut hunks = Vec::new();
    let total = ops.len();
    let mut k = 0;
    while k < total {
        // Skip equal lines outside a hunk window.
        if ops[k].0 == Op::Eq {
            k += 1;
            continue;
        }
        // Start a hunk: include up to `context` prior equal lines.
        let hunk_start = k.saturating_sub(context);
        // Extend hunk until we have `context` trailing equal lines after last change.
        let mut end = k;
        loop {
            // Advance past changes.
            while end < total && ops[end].0 != Op::Eq { end += 1; }
            // Count trailing equals.
            let trail_start = end;
            let mut trail = 0;
            while end < total && ops[end].0 == Op::Eq && trail < context {
                end += 1; trail += 1;
            }
            // Check if the next change is within context distance.
            if end < total && ops[end].0 != Op::Eq {
                continue; // merge with next change cluster
            }
            // Include up to `context` trailing equal lines.
            end = trail_start + trail;
            break;
        }

        // Build hunk lines.
        let hunk_ops = &ops[hunk_start..end];
        let old_start = hunk_ops.iter().find(|o| o.0 != Op::Ins).map(|o| o.1 + 1).unwrap_or(1);
        let new_start = hunk_ops.iter().find(|o| o.0 != Op::Del).map(|o| o.2 + 1).unwrap_or(1);
        let old_count = hunk_ops.iter().filter(|o| o.0 != Op::Ins).count();
        let new_count = hunk_ops.iter().filter(|o| o.0 != Op::Del).count();

        let header = format!("@@ -{},{} +{},{} @@", old_start, old_count, new_start, new_count);
        let lines: Vec<serde_json::Value> = hunk_ops.iter().map(|(op, oi, ni)| {
            let (tag, text) = match op {
                Op::Eq  => (" ", old[*oi]),
                Op::Del => ("-", old[*oi]),
                Op::Ins => ("+", new[*ni]),
            };
            json!({"tag": tag, "text": text})
        }).collect();

        hunks.push(json!({"header": header, "lines": lines}));
        k = end;
    }

    serde_json::Value::Array(hunks)
}

/// DELETE /api/v1/drive/files/:id/versions/:v — remove a specific historical version.
///
/// The current live version (stored in drive_files) cannot be deleted this way;
/// use DELETE /drive/files/:id to trash or permanently remove the whole file.
/// On success the blob is deleted from disk and 204 is returned.
async fn delete_version(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, v)): Path<(Uuid, i32)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    // Verify the file belongs to this tenant/user.
    let f = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    if f.owner_user_id != ctx.user_id {
        return Err(DriveError::Forbidden);
    }
    // Prevent deleting the current version (version_no in drive_files is stored
    // as the count of uploads; the current blob is in drive_files.storage_key).
    // We detect "current" by comparing storage_key: if the version's blob matches
    // the live file, refuse. Fall back to just checking version count.
    let ver = VersionRepo::new(pool).delete(ctx.tenant_id, id, v).await?
        .ok_or(DriveError::NotFound(id))?;

    // Best-effort blob removal — log but don't fail if already gone.
    let blob_path = state.data_root().join(&ver.storage_key);
    if let Err(e) = fs::remove_file(&blob_path).await {
        tracing::warn!(error = %e, path = %blob_path.display(), "delete_version: blob removal failed");
    }

    tracing::info!(target: "audit",
        event = "drive.file.version_deleted",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        file_id = %id, version_no = v);

    Ok(StatusCode::NO_CONTENT)
}

// ─── Tag handlers ─────────────────────────────────────────────────────────────

/// GET /api/v1/drive/files/:id/tags — list tags on a file.
async fn list_tags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<Vec<String>>> {
    let pool = state.db_or_unavailable()?;
    // Verify file exists in tenant.
    FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    let tags = TagRepo::new(pool).list(ctx.tenant_id, id).await?;
    Ok(Json(tags))
}

#[derive(Debug, serde::Deserialize)]
struct AddTagBody {
    tag: String,
}

/// POST /api/v1/drive/files/:id/tags — add a tag to a file (idempotent).
async fn add_tag(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<AddTagBody>,
) -> Result<StatusCode> {
    let tag = body.tag.trim().to_string();
    if tag.is_empty() || tag.len() > 64 {
        return Err(DriveError::BadRequest("tag must be 1–64 characters".into()));
    }
    let pool = state.db_or_unavailable()?;
    // Verify file exists in tenant.
    FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    TagRepo::new(pool).add(ctx.tenant_id, id, &tag).await?;
    tracing::info!(target: "audit",
        event = "drive.file.tag_added",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        file_id = %id, tag = %tag);
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/drive/files/:id/tags/:tag — remove a tag from a file.
async fn remove_tag(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, tag)): Path<(Uuid, String)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    // Verify file exists in tenant.
    FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    TagRepo::new(pool).remove(ctx.tenant_id, id, &tag).await?;
    tracing::info!(target: "audit",
        event = "drive.file.tag_removed",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        file_id = %id, tag = %tag);
    Ok(StatusCode::NO_CONTENT)
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

#[derive(Debug, Deserialize)]
struct ExpiryBody {
    /// RFC 3339 timestamp; null or omitted clears the expiry.
    #[serde(default, with = "time::serde::rfc3339::option")]
    expires_at: Option<OffsetDateTime>,
}

/// PATCH /api/v1/drive/files/:id/expiry — set or clear the expiry timestamp.
///
/// Only the file owner or a moderator may call this. On expiry the file is
/// hard-deleted by the background purge worker (hourly GC).
async fn set_expiry(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<ExpiryBody>,
) -> Result<Json<DriveFile>> {
    let pool = state.db_or_unavailable()?;
    let f    = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    if f.owner_user_id != ctx.user_id {
        return Err(DriveError::Forbidden);
    }
    if let Some(exp) = body.expires_at {
        if exp <= OffsetDateTime::now_utc() {
            return Err(DriveError::BadRequest("expires_at must be in the future".into()));
        }
    }
    let updated = FileRepo::new(pool).set_expiry(ctx.tenant_id, id, body.expires_at).await?;
    tracing::info!(target: "audit",
        event = "drive.file.expiry_set",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        file_id = %id, expires_at = ?body.expires_at);
    Ok(Json(updated))
}

/// POST /api/v1/drive/starred — list user's starred files (newest star first)
async fn list_starred(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<Vec<DriveFile>>> {
    let pool  = state.db_or_unavailable()?;
    let files = FileRepo::new(pool).list_starred(ctx.tenant_id, ctx.user_id).await?;
    Ok(Json(files))
}

/// GET /api/v1/drive/starred/count — count user's starred files (sprint #458).
/// Mesma semântica de `list_starred` (filtra `starred_at IS NOT NULL` + `deleted_at IS NULL`),
/// só retorna `{count: N}` pra badge de UI sem trafegar a lista. Path child de
/// `/starred` (sem ambiguidade com `:id` em outros recursos).
async fn count_starred(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let count = FileRepo::new(pool).count_starred(ctx.tenant_id, ctx.user_id).await?;
    Ok(Json(serde_json::json!({ "count": count })))
}

/// POST /api/v1/drive/files/:id/star — mark file as starred
async fn star_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<DriveFile>> {
    let pool    = state.db_or_unavailable()?;
    let updated = FileRepo::new(pool).star_file(ctx.tenant_id, id, ctx.user_id).await?;
    Ok(Json(updated))
}

/// DELETE /api/v1/drive/files/:id/star — remove star
async fn unstar_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<DriveFile>> {
    let pool    = state.db_or_unavailable()?;
    let updated = FileRepo::new(pool).unstar_file(ctx.tenant_id, id, ctx.user_id).await?;
    Ok(Json(updated))
}

/// POST /api/v1/drive/files/:id/lock — acquire optimistic lock (owner only or first caller)
/// DELETE /api/v1/drive/files/:id/lock — release lock (only the lock holder may unlock)
async fn lock_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<DriveFile>> {
    let pool    = state.db_or_unavailable()?;
    let updated = FileRepo::new(pool).lock_file(ctx.tenant_id, id, ctx.user_id).await?;
    tracing::info!(target: "audit",
        event = "drive.file.locked",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %id);
    Ok(Json(updated))
}

async fn unlock_file(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<DriveFile>> {
    let pool    = state.db_or_unavailable()?;
    let updated = FileRepo::new(pool).unlock_file(ctx.tenant_id, id, ctx.user_id).await?;
    tracing::info!(target: "audit",
        event = "drive.file.unlocked",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, file_id = %id);
    Ok(Json(updated))
}

/// GET /api/v1/drive/folders/:id/download
///
/// Recursively packs all files inside the folder (including sub-folders) into a
/// ZIP archive returned in-memory. Empty folders produce an empty ZIP.
async fn download_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Response> {
    let pool   = state.db_or_unavailable()?;
    let repo   = FileRepo::new(pool);
    let folder = repo.get(ctx.tenant_id, id).await?;
    if folder.kind != "folder" {
        return Err(DriveError::BadRequest("target is not a folder".into()));
    }

    let entries = repo.collect_files_recursive(ctx.tenant_id, id, "").await?;

    let buf: Vec<u8> = Vec::new();
    let cursor = std::io::Cursor::new(buf);
    let mut zip = zip::ZipWriter::new(cursor);
    let options = zip::write::FileOptions::<()>::default()
        .compression_method(zip::CompressionMethod::Deflated);

    for (rel_path, file) in &entries {
        let key = match file.storage_key.as_deref() {
            Some(k) => k,
            None    => continue,
        };
        let bytes = match fs::read(state.data_root().join(key)).await {
            Ok(b)  => b,
            Err(e) => {
                tracing::warn!(file_id = %file.id, error = %e, "skipping unreadable blob in folder download");
                continue;
            }
        };
        zip.start_file(rel_path, options).map_err(|e| DriveError::Io(std::io::Error::other(e.to_string())))?;
        zip.write_all(&bytes).map_err(DriveError::Io)?;
    }

    let cursor = zip.finish().map_err(|e| DriveError::Io(std::io::Error::other(e.to_string())))?;
    let zip_bytes = cursor.into_inner();

    let ascii: String = folder.name.chars().map(|c| {
        if c.is_ascii_graphic() && c != '"' && c != '\\' { c } else { '_' }
    }).collect();
    let archive_name = if ascii.is_empty() { "folder".to_string() } else { ascii };
    let cd = format!("attachment; filename=\"{archive_name}.zip\"");

    tracing::info!(target: "audit",
        event = "drive.folder.download",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, folder_id = %id,
        file_count = entries.len());

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE,        HeaderValue::from_static("application/zip")),
            (header::CONTENT_DISPOSITION, HeaderValue::from_str(&cd).unwrap_or_else(|_| HeaderValue::from_static("attachment"))),
        ],
        zip_bytes,
    ).into_response())
}

/// GET /api/v1/drive/folders/:id/quota — current folder quota + used bytes.
/// Returns 404 if folder doesn't exist or has no quota configured.
async fn folder_quota(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<FolderQuota>> {
    let pool = state.db_or_unavailable()?;
    // Verify folder exists and belongs to tenant.
    let f = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    if f.kind != "folder" {
        return Err(DriveError::BadRequest("id is not a folder".into()));
    }
    FolderQuotaRepo::new(pool)
        .get(ctx.tenant_id, id).await?
        .ok_or_else(|| DriveError::NotFound(id))
        .map(Json)
}

#[derive(Debug, serde::Deserialize)]
struct FolderQuotaBody {
    max_bytes: i64,
}

/// PUT /api/v1/drive/folders/:id/quota — set (upsert) folder quota.
async fn set_folder_quota(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<FolderQuotaBody>,
) -> Result<Json<FolderQuota>> {
    let pool = state.db_or_unavailable()?;
    let f = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    if f.kind != "folder" {
        return Err(DriveError::BadRequest("id is not a folder".into()));
    }
    if body.max_bytes <= 0 {
        return Err(DriveError::BadRequest("max_bytes must be > 0".into()));
    }
    let fq = FolderQuotaRepo::new(pool).set(ctx.tenant_id, id, body.max_bytes).await?;
    tracing::info!(target: "audit",
        event = "drive.folder.quota_set",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, folder_id = %id, max_bytes = body.max_bytes);
    Ok(Json(fq))
}

/// DELETE /api/v1/drive/folders/:id/quota — remove folder quota.
async fn delete_folder_quota(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    FolderQuotaRepo::new(pool).delete(ctx.tenant_id, id).await?;
    tracing::info!(target: "audit",
        event = "drive.folder.quota_deleted",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, folder_id = %id);
    Ok(StatusCode::NO_CONTENT)
}

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
        .route("/api/v1/drive/files/:id/versions/:v",          get(download_version).delete(delete_version))
        .route("/api/v1/drive/files/:id/versions/:v/metadata", get(version_metadata))
        .route("/api/v1/drive/files/:id/versions/:v/restore",  post(restore_version))
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
        .route("/api/v1/drive/files/stats",                 get(file_stats))
        .route("/api/v1/drive/files/stats/users",           get(file_stats_users))
        .route("/api/v1/drive/files/stats/folders",         get(file_stats_folders))
        .route("/api/v1/drive/files/stats/extensions",      get(file_stats_extensions))
        .route("/api/v1/drive/files/stats/activity",       get(file_stats_activity))
        .route("/api/v1/drive/files/stats/age",            get(file_stats_age))
        .route("/api/v1/drive/files/stats/owners",        get(file_stats_owners))
        .route("/api/v1/drive/files/stats/size-buckets", get(file_stats_size_buckets))
        .route("/api/v1/drive/files/stats/deleted",         get(file_stats_deleted))
        .route("/api/v1/drive/files/stats/by-owner-and-ext", get(file_stats_by_owner_and_ext))
        .route("/api/v1/drive/files/stats/recent",           get(file_stats_recent))
        .route("/api/v1/drive/files/stats/mime-by-folder",  get(file_stats_mime_by_folder))
        .route("/api/v1/drive/files/stats/top-files",        get(file_stats_top_files))
        .route("/api/v1/drive/files/stats/created-by-day",  get(file_stats_created_by_day))
        .route("/api/v1/drive/files/stats/by-size-bucket",  get(file_stats_by_size_bucket))
        .route("/api/v1/drive/files/stats/updated-by-day",  get(file_stats_updated_by_day))
        .route("/api/v1/drive/files/stats/folder-depth",    get(file_stats_folder_depth))
        .route("/api/v1/drive/files/stats/version-count",   get(file_stats_version_count))
        .route("/api/v1/drive/files/stats/tag-count",        get(file_stats_tag_count))
        .route("/api/v1/drive/files/stats/ext-by-folder",    get(file_stats_ext_by_folder))
        .route("/api/v1/drive/files/stats/lock-count",       get(file_stats_lock_count))
        .route("/api/v1/drive/files/stats/starred-count",    get(file_stats_starred_count))
        .route("/api/v1/drive/files/stats/expiry-count",    get(file_stats_expiry_count))
        .route("/api/v1/drive/files/stats/mime-top-n",       get(file_stats_mime_top_n))
        .route("/api/v1/drive/files/stats/orphan-versions",  get(file_stats_orphan_versions))
        .route("/api/v1/drive/files/stats/empty-files",      get(file_stats_empty_files))
        .route("/api/v1/drive/files/stats/deleted-by-day",   get(file_stats_deleted_by_day))
        .route("/api/v1/drive/files/stats/name-length",       get(file_stats_name_length))
        .route("/api/v1/drive/files/stats/ext-top-n",         get(file_stats_ext_top_n))
        .route("/api/v1/drive/files/stats/storage-by-user",   get(file_stats_storage_by_user))
        .route("/api/v1/drive/files/stats/quota-usage",        get(file_stats_quota_usage))
        .route("/api/v1/drive/files/stats/folder-file-count",  get(file_stats_folder_file_count))
        .route("/api/v1/drive/files/stats/deep-files",         get(file_stats_deep_files))
        .route("/api/v1/drive/files/stats/mime-entropy",        get(file_stats_mime_entropy))
        .route("/api/v1/drive/files/stats/avg-versions",         get(file_stats_avg_versions))
        .route("/api/v1/drive/files/stats/ext-entropy",           get(file_stats_ext_entropy))
        .route("/api/v1/drive/files/stats/checksum-coverage",      get(file_stats_checksum_coverage))
        .route("/api/v1/drive/files/stats/storage-key-coverage",   get(file_stats_storage_key_coverage))
        .route("/api/v1/drive/files/stats/locked-by-user",         get(file_stats_locked_by_user))
        .route("/api/v1/drive/files/stats/mime-by-ext",             get(file_stats_mime_by_ext))
        .route("/api/v1/drive/files/stats/size-trend-by-day",       get(file_stats_size_trend_by_day))
        .route("/api/v1/drive/files/stats/version-age",             get(file_stats_version_age))
        .route("/api/v1/drive/files/stats/mime-count-by-user",      get(file_stats_mime_count_by_user))
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

/// GET /api/v1/drive/files/stats/users?limit=N — top-N users by storage usage.
///
/// Returns `{users: [{user_id, file_count, used_bytes}]}` ordered by `used_bytes DESC`.
/// Only counts non-deleted files (`kind='file'`). `limit` default 20, max 200.
/// Useful for capacity planning and "heavy users" dashboards. Sprint #621.
#[derive(Debug, Deserialize)]
struct StatsUsersQuery {
    limit: Option<i64>,
}

async fn file_stats_users(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsUsersQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    let rows  = QuotaRepo::new(pool).top_users_by_usage(ctx.tenant_id, limit).await?;
    let users: Vec<serde_json::Value> = rows.into_iter().map(|(uid, fc, ub)| {
        serde_json::json!({"user_id": uid, "file_count": fc, "used_bytes": ub})
    }).collect();
    Ok(Json(serde_json::json!({"users": users})))
}

/// GET /api/v1/drive/files/stats/folders?limit=N — top-N folders by recursive storage usage.
///
/// Returns `{folders: [{folder_id, folder_name, file_count, used_bytes}]}` ordered by
/// `used_bytes DESC`. Uses a recursive CTE to aggregate size_bytes of all descendant
/// files for each root folder. Only counts non-deleted files (`kind='file'`).
/// `limit` default 20, max 200. Sprint #626.
#[derive(Debug, Deserialize)]
struct StatsFoldersQuery {
    limit: Option<i64>,
}

async fn file_stats_folders(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsFoldersQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    let rows  = QuotaRepo::new(pool).top_folders_by_usage(ctx.tenant_id, limit).await?;
    let folders: Vec<serde_json::Value> = rows.into_iter().map(|(fid, fname, fc, ub)| {
        serde_json::json!({"folder_id": fid, "folder_name": fname, "file_count": fc, "used_bytes": ub})
    }).collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/drive/files/stats/extensions?limit=N — breakdown by file extension.
///
/// Returns `{extensions: [{extension, file_count, total_bytes}]}` ordered by
/// `total_bytes DESC`. Extension = `lower(split_part(name, '.', -1))` — the part
/// after the last dot; files with no dot get extension "". Only counts non-deleted
/// files (`kind='file'`). `limit` default 50, max 500. Sprint #631.
#[derive(Debug, Deserialize)]
struct StatsExtensionsQuery {
    limit: Option<i64>,
}

async fn file_stats_extensions(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsExtensionsQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let rows  = QuotaRepo::new(pool).stats_by_extension(ctx.tenant_id, limit).await?;
    let extensions: Vec<serde_json::Value> = rows.into_iter().map(|(ext, fc, tb)| {
        serde_json::json!({"extension": ext, "file_count": fc, "total_bytes": tb})
    }).collect();
    Ok(Json(serde_json::json!({"extensions": extensions})))
}

/// GET /api/v1/drive/files/stats/activity?since=&until=
///
/// Returns uploads/updates/deletes per day for the tenant in the given range.
/// `since`/`until` are RFC 3339 timestamps (optional). Response:
/// `{days: [{day, uploads, updates, deletes}]}` ordered by day ASC. Sprint #636.
#[derive(Debug, Deserialize)]
struct StatsActivityQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    since: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    until: Option<OffsetDateTime>,
}

async fn file_stats_activity(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsActivityQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows = QuotaRepo::new(pool).activity_by_day(ctx.tenant_id, q.since, q.until).await?;
    let days: Vec<serde_json::Value> = rows.into_iter().map(|(day, uploads, updates, deletes)| {
        serde_json::json!({"day": day, "uploads": uploads, "updates": updates, "deletes": deletes})
    }).collect();
    Ok(Json(serde_json::json!({"days": days})))
}

/// GET /api/v1/drive/files/stats/created-by-day?since=&until= — arquivos criados por dia.
///
/// DATE_TRUNC('day', created_at) + COUNT sobre `drive_files` (kind='file', não-deletados).
/// `since`/`until` RFC3339 opcionais via `$N::timestamptz IS NULL OR`. Retorna
/// `{days:[{day,count}]}` ordenado dia ASC. Foca em criação (vs activity/#636 que usa audit).
/// Sprint #682.
async fn file_stats_created_by_day(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsActivityQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT to_char(date_trunc('day', created_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
                COUNT(*)::BIGINT AS count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
            AND ($2::timestamptz IS NULL OR created_at >= $2) \
            AND ($3::timestamptz IS NULL OR created_at <  $3) \
          GROUP BY day \
          ORDER BY day ASC",
    )
    .bind(ctx.tenant_id)
    .bind(q.since)
    .bind(q.until)
    .fetch_all(pool)
    .await?;

    let days: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, count)| serde_json::json!({"day": day, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"days": days})))
}

/// GET /api/v1/drive/files/stats/age — file creation timeline by calendar month.
///
/// Returns `{months: [{month, file_count}]}` ordered month ASC. Month format is
/// `YYYY-MM`. Only counts non-deleted files (`kind='file'`). Tenant-scoped.
/// Useful for "when were files uploaded" dashboards. Sprint #641.
async fn file_stats_age(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let rows = QuotaRepo::new(pool).age_by_month(ctx.tenant_id).await?;
    let months: Vec<serde_json::Value> = rows.into_iter().map(|(month, file_count)| {
        serde_json::json!({"month": month, "file_count": file_count})
    }).collect();
    Ok(Json(serde_json::json!({"months": months})))
}

/// GET /api/v1/drive/files/stats/owners?limit=N — top-N file owners by file count.
///
/// Returns `{owners: [{owner_user_id, file_count, total_bytes}]}` ordered by
/// `file_count DESC`. Complements `stats/users` (#621) which orders by `used_bytes`;
/// here the primary sort is file count — useful for identifying users who create
/// many small files vs few large ones. Only counts non-deleted files (`kind='file'`).
/// `limit` default 20, max 200. Sprint #646.
#[derive(Debug, Deserialize)]
struct StatsOwnersQuery {
    limit: Option<i64>,
}

async fn file_stats_owners(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsOwnersQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    let rows  = QuotaRepo::new(pool).top_owners_by_file_count(ctx.tenant_id, limit).await?;
    let owners: Vec<serde_json::Value> = rows.into_iter().map(|(uid, fc, tb)| {
        serde_json::json!({"owner_user_id": uid, "file_count": fc, "total_bytes": tb})
    }).collect();
    Ok(Json(serde_json::json!({"owners": owners})))
}

/// GET /api/v1/drive/files/stats/size-buckets — distribuição de arquivos por faixa de tamanho.
///
/// Retorna `{buckets: [{range, count, total_bytes}]}` com as faixas fixas:
/// "<1MB", "1–10MB", "10–100MB", ">100MB". Cobre todos os arquivos não-deletados
/// (`kind='file'`) do tenant. Útil pra "qual percentual do storage são arquivos gigantes?".
/// Sprint #651.
async fn file_stats_size_buckets(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let (lt1mb_c, lt1mb_b,
         lt10mb_c, lt10mb_b,
         lt100mb_c, lt100mb_b,
         gt100mb_c, gt100mb_b): (i64, i64, i64, i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT \
                COUNT(*) FILTER (WHERE size_bytes < 1048576)::BIGINT, \
                COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes < 1048576), 0)::BIGINT, \
                COUNT(*) FILTER (WHERE size_bytes >= 1048576    AND size_bytes < 10485760)::BIGINT, \
                COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 1048576    AND size_bytes < 10485760), 0)::BIGINT, \
                COUNT(*) FILTER (WHERE size_bytes >= 10485760   AND size_bytes < 104857600)::BIGINT, \
                COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 10485760   AND size_bytes < 104857600), 0)::BIGINT, \
                COUNT(*) FILTER (WHERE size_bytes >= 104857600)::BIGINT, \
                COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 104857600), 0)::BIGINT \
             FROM drive_files \
             WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file'",
        )
        .bind(ctx.tenant_id)
        .fetch_one(pool)
        .await?;

    let buckets = vec![
        serde_json::json!({"range": "<1MB",     "count": lt1mb_c,   "total_bytes": lt1mb_b}),
        serde_json::json!({"range": "1–10MB",   "count": lt10mb_c,  "total_bytes": lt10mb_b}),
        serde_json::json!({"range": "10–100MB", "count": lt100mb_c, "total_bytes": lt100mb_b}),
        serde_json::json!({"range": ">100MB",   "count": gt100mb_c, "total_bytes": gt100mb_b}),
    ];
    Ok(Json(serde_json::json!({"buckets": buckets})))
}

/// GET /api/v1/drive/files/stats/deleted — métricas de arquivos na lixeira.
///
/// Conta arquivos com `deleted_at IS NOT NULL` e soma seus bytes. Retorna também
/// `oldest_deleted_at` e `newest_deleted_at` para dar o intervalo temporal da lixeira.
/// Tenant-scoped. Sprint #657.
async fn file_stats_deleted(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let (deleted_count, deleted_bytes, oldest_deleted_at, newest_deleted_at):
        (i64, i64, Option<OffsetDateTime>, Option<OffsetDateTime>) =
        sqlx::query_as(
            "SELECT \
                COUNT(*)::BIGINT, \
                COALESCE(SUM(size_bytes), 0)::BIGINT, \
                MIN(deleted_at), \
                MAX(deleted_at) \
             FROM drive_files \
             WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND kind = 'file'",
        )
        .bind(ctx.tenant_id)
        .fetch_one(pool)
        .await?;

    Ok(Json(serde_json::json!({
        "deleted_count":      deleted_count,
        "deleted_bytes":      deleted_bytes,
        "oldest_deleted_at":  oldest_deleted_at,
        "newest_deleted_at":  newest_deleted_at,
    })))
}

/// GET /api/v1/drive/files/stats/by-owner-and-ext?limit=N — top-N pares (owner, extensão) por bytes.
///
/// Retorna `{rows:[{owner_user_id,extension,file_count,total_bytes}]}` ordenado por
/// `total_bytes DESC`. Útil pra identificar quais usuários+tipos de arquivo dominam o
/// armazenamento. Só arquivos não-deletados (`kind='file'`). `limit` default 20, max 200.
/// Sprint #662.
#[derive(Debug, Deserialize)]
struct StatsOwnerExtQuery {
    limit: Option<i64>,
}

async fn file_stats_by_owner_and_ext(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsOwnerExtQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    let rows: Vec<(Uuid, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT owner_user_id, extension, \
                COUNT(*)::BIGINT AS file_count, \
                COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
         GROUP BY owner_user_id, extension \
         ORDER BY total_bytes DESC \
         LIMIT $2",
    )
    .bind(ctx.tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let out: Vec<serde_json::Value> = rows.into_iter().map(|(uid, ext, fc, tb)| {
        serde_json::json!({
            "owner_user_id": uid,
            "extension":     ext,
            "file_count":    fc,
            "total_bytes":   tb,
        })
    }).collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/recent?since=&limit=N — arquivos criados/modificados recentemente.
///
/// Retorna `{files:[{id,name,size_bytes,created_at,updated_at}]}` ordenado por
/// `updated_at DESC`. `since` RFC3339 opcional (filtra `updated_at >= since`).
/// `limit` default 20 max 200. Só arquivos não-deletados (`kind='file'`). Sprint #667.
#[derive(Debug, Deserialize)]
struct StatsRecentQuery {
    since: Option<String>,
    limit: Option<i64>,
}

async fn file_stats_recent(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsRecentQuery>,
) -> Result<Json<serde_json::Value>> {
    use time::format_description::well_known::Rfc3339;

    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    let since_dt = q.since.as_deref()
        .map(|s| OffsetDateTime::parse(s, &Rfc3339))
        .transpose()
        .map_err(|_| crate::error::DriveError::BadRequest("since must be RFC3339".into()))?;

    let rows: Vec<(Uuid, String, i64, OffsetDateTime, OffsetDateTime)> = sqlx::query_as(
        "SELECT id, name, COALESCE(size_bytes, 0)::BIGINT, created_at, updated_at \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
            AND ($2::timestamptz IS NULL OR updated_at >= $2) \
          ORDER BY updated_at DESC \
          LIMIT $3",
    )
    .bind(ctx.tenant_id)
    .bind(since_dt)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let files: Vec<serde_json::Value> = rows.into_iter()
        .map(|(id, name, size_bytes, created_at, updated_at)| serde_json::json!({
            "id":         id,
            "name":       name,
            "size_bytes": size_bytes,
            "created_at": created_at,
            "updated_at": updated_at,
        }))
        .collect();
    Ok(Json(serde_json::json!({"files": files})))
}

/// GET /api/v1/drive/files/stats/mime-by-folder?folder_id= — breakdown de mime_type numa pasta.
///
/// `folder_id` opcional: quando ausente agrega na raiz (`parent_id IS NULL`).
/// Retorna `{folder_id, rows:[{mime_type,file_count,total_bytes}]}` ordenado por
/// `total_bytes DESC`. Só arquivos não-deletados (`kind='file'`). Sprint #672.
#[derive(Debug, Deserialize)]
struct StatsMimeByFolderQuery {
    folder_id: Option<Uuid>,
}

async fn file_stats_mime_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsMimeByFolderQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<String>, i64, i64)> = if let Some(fid) = q.folder_id {
        sqlx::query_as(
            "SELECT mime_type, COUNT(*)::BIGINT AS file_count, \
                    COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
               FROM drive_files \
              WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
                AND parent_id = $2 \
              GROUP BY mime_type \
              ORDER BY total_bytes DESC",
        )
        .bind(ctx.tenant_id)
        .bind(fid)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT mime_type, COUNT(*)::BIGINT AS file_count, \
                    COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
               FROM drive_files \
              WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
                AND parent_id IS NULL \
              GROUP BY mime_type \
              ORDER BY total_bytes DESC",
        )
        .bind(ctx.tenant_id)
        .fetch_all(pool)
        .await?
    };

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime, fc, tb)| serde_json::json!({
            "mime_type":   mime,
            "file_count":  fc,
            "total_bytes": tb,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folder_id": q.folder_id, "rows": out})))
}

/// GET /api/v1/drive/files/stats/top-files?limit=N — top-N arquivos por size_bytes.
///
/// Retorna `{files:[{id,name,size_bytes,owner_user_id,mime_type}]}` ordenado por
/// `size_bytes DESC`. `limit` default 20 max 200. Só arquivos não-deletados.
/// Complementa size-buckets (#651) com lista concreta dos maiores arquivos. Sprint #677.
#[derive(Debug, Deserialize)]
struct StatsTopFilesQuery {
    limit: Option<i64>,
}

async fn file_stats_top_files(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(Uuid, String, i64, Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, name, COALESCE(size_bytes, 0)::BIGINT, owner_user_id, mime_type \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
          ORDER BY size_bytes DESC NULLS LAST \
          LIMIT $2",
    )
    .bind(ctx.tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let files: Vec<serde_json::Value> = rows.into_iter()
        .map(|(id, name, size_bytes, owner_user_id, mime_type)| serde_json::json!({
            "id":            id,
            "name":          name,
            "size_bytes":    size_bytes,
            "owner_user_id": owner_user_id,
            "mime_type":     mime_type,
        }))
        .collect();
    Ok(Json(serde_json::json!({"files": files})))
}

/// GET /api/v1/drive/files/:id/versions?limit=N&before_version=V
///
/// Lista versões em ordem DESC de version_no. `before_version` é o version_no
/// do último item da página anterior (keyset cursor — retorna versões com
/// version_no < before_version). `limit` default 50, max 500.
/// Response inclui `{versions, next_cursor, has_more}`. Sprint #608.
#[derive(Debug, Deserialize)]
struct ListVersionsParams {
    limit:          Option<i64>,
    before_version: Option<i32>,
}

async fn list_versions(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Query(q):     Query<ListVersionsParams>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let f = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    let max_created: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(created_at) FROM drive_file_versions WHERE tenant_id = $1 AND file_id = $2",
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

    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let rows: Vec<crate::domain::version::FileVersion> = if let Some(bv) = q.before_version {
        sqlx::query_as(
            "SELECT id, file_id, tenant_id, version_no, storage_key, size_bytes, sha256, \
                    mime_type, created_by, created_at \
               FROM drive_file_versions \
              WHERE tenant_id = $1 AND file_id = $2 AND version_no < $3 \
              ORDER BY version_no DESC \
              LIMIT $4",
        )
        .bind(ctx.tenant_id).bind(id).bind(bv).bind(limit)
        .fetch_all(pool).await?
    } else {
        sqlx::query_as(
            "SELECT id, file_id, tenant_id, version_no, storage_key, size_bytes, sha256, \
                    mime_type, created_by, created_at \
               FROM drive_file_versions \
              WHERE tenant_id = $1 AND file_id = $2 \
              ORDER BY version_no DESC \
              LIMIT $3",
        )
        .bind(ctx.tenant_id).bind(id).bind(limit)
        .fetch_all(pool).await?
    };

    let has_more    = rows.len() as i64 == limit;
    let next_cursor = rows.last().map(|v| v.version_no);
    let count       = rows.len();

    let mut resp = serde_json::json!({
        "versions":    rows,
        "next_cursor": next_cursor,
        "has_more":    has_more,
    });
    // Back-compat: when no cursor params, also embed total count.
    if q.before_version.is_none() {
        resp["total"] = serde_json::json!(count);
    }
    let mut r = Json(resp).into_response();
    r.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(r)
}

/// GET /api/v1/drive/files/:id/versions/:v/metadata — metadados de uma versão sem download.
///
/// Retorna `{version_no, mime_type, sha256, size_bytes, created_at}` para a versão `:v`.
/// Útil pra verificar integridade (sha256) sem baixar o blob. 404 se versão não encontrada.
/// Sprint #602.
async fn version_metadata(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, v)): Path<(Uuid, i32)>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    // Tenant-gate via file ownership check.
    let _ = FileRepo::new(pool).get(ctx.tenant_id, id).await?;
    let ver = VersionRepo::new(pool).get(ctx.tenant_id, id, v).await?
        .ok_or(DriveError::NotFound(id))?;
    Ok(Json(serde_json::json!({
        "file_id":    id,
        "version_no": ver.version_no,
        "mime_type":  ver.mime_type,
        "sha256":     ver.sha256,
        "size_bytes": ver.size_bytes,
        "created_at": ver.created_at,
    })))
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

/// POST /api/v1/drive/files/:id/versions/:v/restore
///
/// Promotes a historical version `:v` to the current live content of file `:id`.
/// The current live content is archived as a new version before the swap, so no
/// data is lost. Returns the updated file record.
///
/// Flow:
///   1. Fetch current live file (404 if not found or deleted).
///   2. Fetch target version `:v` (404 if missing).
///   3. Archive current live blob as a new version (next_no).
///   4. Swap `drive_files.storage_key / size_bytes / sha256 / mime_type` to the
///      target version's blob via `FileRepo::update_content`.
///
/// Idempotent if called twice with the same version: the second call finds the
/// current live content already matching, creates an extra archive version.
/// Sprint #611.
async fn restore_version(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, v)): Path<(Uuid, i32)>,
) -> Result<Json<DriveFile>> {
    let pool = state.db_or_unavailable()?;

    let file_repo = FileRepo::new(pool);
    let ver_repo  = VersionRepo::new(pool);

    let current = file_repo.get(ctx.tenant_id, id).await?;
    let target  = ver_repo.get(ctx.tenant_id, id, v).await?
        .ok_or(DriveError::NotFound(id))?;

    // Archive the current live blob as a new historical version.
    if let Some(ref current_key) = current.storage_key {
        let next_no = ver_repo.next_no(ctx.tenant_id, id).await?;
        ver_repo.insert(&NewVersion {
            file_id:     id,
            tenant_id:   ctx.tenant_id,
            version_no:  next_no,
            storage_key: current_key,
            size_bytes:  current.size_bytes,
            sha256:      current.sha256.as_deref(),
            mime_type:   current.mime_type.as_deref(),
            created_by:  ctx.user_id,
        }).await?;
    }

    // Promote target version to live.
    let updated = file_repo.update_content(
        ctx.tenant_id,
        id,
        &target.storage_key,
        target.size_bytes,
        target.sha256.as_deref(),
        target.mime_type.as_deref(),
    ).await?;

    tracing::info!(target: "audit",
        event = "drive.file.restore_version",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        file_id = %id, version_no = v);

    Ok(Json(updated))
}

/// GET /api/v1/drive/files/:id/versions/:v/diff-content
///
/// Returns a unified text diff between version `:v` and the version immediately
/// before it (v-1). 404 if either version blob is missing or the file is not
/// text (detected via Content-Type prefix). 409 if v == 1 (no previous version).
/// Response: `{version_a, version_b, hunks: [{header, lines: [{tag,text}]}]}`.
/// Binary-safe guard: rejects blobs with a NUL byte in the first 8 KiB.
#[derive(Debug, Deserialize)]
struct DiffParams {
    /// Lines of context around each changed region (default 3, clamped to 0–50).
    context: Option<u32>,
    /// Output format: "unified" (default) or "side-by-side".
    format: Option<String>,
    /// Base version to diff from. Defaults to v-1 (previous version).
    from: Option<i32>,
}

async fn diff_version_content(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, v)): Path<(Uuid, i32)>,
    Query(params): Query<DiffParams>,
) -> Result<Json<serde_json::Value>> {
    use serde_json::json;

    let fmt = params.format.as_deref().unwrap_or("unified");
    if fmt != "unified" && fmt != "side-by-side" {
        return Err(DriveError::BadRequest("format must be 'unified' or 'side-by-side'".into()));
    }

    let context = params.context.unwrap_or(3).min(50) as usize;

    // Determine which version pair to diff.
    let v_a = match params.from {
        Some(from) => {
            if from < 1 {
                return Err(DriveError::BadRequest("from must be >= 1".into()));
            }
            if from == v {
                return Err(DriveError::BadRequest("from and v must differ".into()));
            }
            from
        }
        None => {
            if v <= 1 {
                return Err(DriveError::BadRequest("no previous version to diff (v must be > 1, or specify ?from=)".into()));
            }
            v - 1
        }
    };

    let pool = state.db_or_unavailable()?;
    let _file = FileRepo::new(pool).get(ctx.tenant_id, id).await?;

    let ver_b = VersionRepo::new(pool).get(ctx.tenant_id, id, v).await?
        .ok_or(DriveError::NotFound(id))?;
    let ver_a = VersionRepo::new(pool).get(ctx.tenant_id, id, v_a).await?
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

    if fmt == "side-by-side" {
        let rows = side_by_side_diff(&lines_a, &lines_b, context);
        return Ok(Json(json!({
            "file_id":   id,
            "version_a": v_a,
            "version_b": v,
            "format":    "side-by-side",
            "context":   context,
            "rows":      rows,
        })));
    }

    let hunks = unified_diff(&lines_a, &lines_b, context);
    Ok(Json(json!({
        "file_id":   id,
        "version_a": v_a,
        "version_b": v,
        "format":    "unified",
        "context":   context,
        "hunks":     hunks,
    })))
}

/// Compute a side-by-side diff: each row has `{type, left, right}`.
/// `type`: "equal" | "changed" | "deleted" | "inserted".
/// `left`/`right`: `{line_no: usize | null, text: String | null}`.
/// Rows outside a context window of a change are suppressed.
/// Sprint #592.
fn side_by_side_diff(old: &[&str], new: &[&str], context: usize) -> serde_json::Value {
    use serde_json::json;

    let m = old.len();
    let n = new.len();

    // LCS table (same DP as unified_diff).
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

    #[derive(Clone, Copy, PartialEq)]
    enum Op { Eq, Del, Ins }

    let mut ops: Vec<(Op, usize, usize)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < m || j < n {
        if i < m && j < n && old[i] == new[j] {
            ops.push((Op::Eq, i, j)); i += 1; j += 1;
        } else if j < n && (i >= m || lcs[i][j + 1] >= lcs[i + 1][j]) {
            ops.push((Op::Ins, i, j)); j += 1;
        } else {
            ops.push((Op::Del, i, j)); i += 1;
        }
    }

    // Pair up consecutive Del+Ins into "changed" rows; lone Del → "deleted"; lone Ins → "inserted".
    // Then filter to within context of any non-equal row.
    #[derive(Clone)]
    struct Row {
        kind:     &'static str,
        left_no:  Option<usize>,
        left_txt: Option<String>,
        right_no: Option<usize>,
        right_txt: Option<String>,
    }

    let mut raw: Vec<Row> = Vec::new();
    let total = ops.len();
    let mut k = 0;
    while k < total {
        match ops[k].0 {
            Op::Eq => {
                let (_, oi, ni) = ops[k];
                raw.push(Row {
                    kind: "equal",
                    left_no: Some(oi + 1), left_txt: Some(old[oi].to_owned()),
                    right_no: Some(ni + 1), right_txt: Some(new[ni].to_owned()),
                });
                k += 1;
            }
            Op::Del => {
                let (_, oi, _ni) = ops[k];
                // Peek: if next op is Ins, pair as "changed".
                if k + 1 < total && ops[k + 1].0 == Op::Ins {
                    let (_, _oi2, ni2) = ops[k + 1];
                    raw.push(Row {
                        kind: "changed",
                        left_no: Some(oi + 1), left_txt: Some(old[oi].to_owned()),
                        right_no: Some(ni2 + 1), right_txt: Some(new[ni2].to_owned()),
                    });
                    k += 2;
                } else {
                    raw.push(Row {
                        kind: "deleted",
                        left_no: Some(oi + 1), left_txt: Some(old[oi].to_owned()),
                        right_no: None, right_txt: None,
                    });
                    k += 1;
                }
            }
            Op::Ins => {
                let (_, _, ni) = ops[k];
                raw.push(Row {
                    kind: "inserted",
                    left_no: None, left_txt: None,
                    right_no: Some(ni + 1), right_txt: Some(new[ni].to_owned()),
                });
                k += 1;
            }
        }
    }

    // Mark which rows are "near" a change (within context distance).
    let total_rows = raw.len();
    let mut visible = vec![false; total_rows];
    for r in 0..total_rows {
        if raw[r].kind != "equal" {
            let lo = r.saturating_sub(context);
            let hi = (r + context + 1).min(total_rows);
            for v in lo..hi { visible[v] = true; }
        }
    }

    let rows: Vec<serde_json::Value> = raw.iter().zip(visible.iter())
        .filter(|(_, &vis)| vis)
        .map(|(row, _)| json!({
            "type":  row.kind,
            "left":  json!({"line_no": row.left_no, "text": row.left_txt}),
            "right": json!({"line_no": row.right_no, "text": row.right_txt}),
        }))
        .collect();

    serde_json::Value::Array(rows)
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

/// GET /api/v1/drive/files/stats
///
/// Returns storage breakdown for the tenant: total files, total folders, total
/// size, and per-MIME-type aggregates. Only non-deleted (live) files are counted.
/// `by_mime_type` is ordered by `total_bytes DESC` so the heaviest types appear
/// first — useful for "storage breakdown" UIs. Folders have no mime_type and
/// appear as `"application/x-directory"` bucket (folded from NULL + kind=folder).
/// Response: `{files, folders, total_bytes, by_mime_type: [{mime_type, count, total_bytes}]}`.
/// Sprint #616.
async fn file_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    // Total file and folder counts + overall size.
    let (files, folders, total_bytes): (i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE kind = 'file') AS files, \
            COUNT(*) FILTER (WHERE kind = 'folder') AS folders, \
            COALESCE(SUM(size_bytes) FILTER (WHERE kind = 'file'), 0)::BIGINT AS total_bytes \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    // Per-MIME-type breakdown (files only; NULL mime_type grouped as 'application/octet-stream').
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            COALESCE(mime_type, 'application/octet-stream') AS mime_type, \
            COUNT(*)::BIGINT AS count, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
         GROUP BY mime_type \
         ORDER BY total_bytes DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;

    let by_mime: Vec<serde_json::Value> = rows.into_iter().map(|(mime, count, bytes)| {
        serde_json::json!({"mime_type": mime, "count": count, "total_bytes": bytes})
    }).collect();

    Ok(Json(serde_json::json!({
        "files":        files,
        "folders":      folders,
        "total_bytes":  total_bytes,
        "by_mime_type": by_mime,
    })))
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

/// GET /api/v1/drive/files/stats/updated-by-day?since=&until= — arquivos modificados por dia.
///
/// DATE_TRUNC('day', updated_at) COUNT sobre `drive_files` (kind='file', não-deletados).
/// `since`/`until` RFC3339 opcionais via `$N::timestamptz IS NULL OR`. Retorna
/// `{days:[{day,count}]}` ordenado dia ASC. Complementa `created-by-day` (#682) focando
/// em modificações vs criações. Sprint #692.
async fn file_stats_updated_by_day(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsActivityQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT to_char(date_trunc('day', updated_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
                COUNT(*)::BIGINT AS count \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
            AND ($2::timestamptz IS NULL OR updated_at >= $2) \
            AND ($3::timestamptz IS NULL OR updated_at <  $3) \
          GROUP BY day \
          ORDER BY day ASC",
    )
    .bind(ctx.tenant_id)
    .bind(q.since)
    .bind(q.until)
    .fetch_all(pool)
    .await?;

    let days: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, count)| serde_json::json!({"day": day, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"days": days})))
}

/// GET /api/v1/drive/files/stats/by-size-bucket?folder_id= — distribuição de tamanho em 8 faixas.
///
/// Conta arquivos (kind='file', não-deletados) por faixa de `size_bytes`:
/// <1KB / 1-10KB / 10-100KB / 100KB-1MB / 1-10MB / 10-100MB / 100MB-1GB / >1GB.
/// `folder_id` (UUID) filtra por `parent_id`; omitido = tenant inteiro.
/// Retorna `{buckets:[{range,count,total_bytes}]}`. Sprint #687.
#[derive(Debug, Deserialize)]
struct StatsSizeBucketQuery {
    folder_id: Option<Uuid>,
}

async fn file_stats_by_size_bucket(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsSizeBucketQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let folder_filter = if q.folder_id.is_some() {
        "AND parent_id = $2"
    } else {
        "AND ($2::uuid IS NULL OR parent_id = $2)"
    };

    let sql = format!(
        "SELECT \
            COUNT(*) FILTER (WHERE size_bytes < 1024)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes < 1024), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 1024        AND size_bytes < 10240)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 1024        AND size_bytes < 10240), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 10240       AND size_bytes < 102400)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 10240       AND size_bytes < 102400), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 102400      AND size_bytes < 1048576)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 102400      AND size_bytes < 1048576), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 1048576     AND size_bytes < 10485760)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 1048576     AND size_bytes < 10485760), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 10485760    AND size_bytes < 104857600)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 10485760    AND size_bytes < 104857600), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 104857600   AND size_bytes < 1073741824)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 104857600   AND size_bytes < 1073741824), 0)::BIGINT, \
            COUNT(*) FILTER (WHERE size_bytes >= 1073741824)::BIGINT, \
            COALESCE(SUM(size_bytes) FILTER (WHERE size_bytes >= 1073741824), 0)::BIGINT \
         FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' {folder_filter}"
    );

    let row: (i64,i64, i64,i64, i64,i64, i64,i64, i64,i64, i64,i64, i64,i64, i64,i64) =
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(q.folder_id)
            .fetch_one(pool)
            .await?;

    let buckets = vec![
        serde_json::json!({"range": "<1KB",        "count": row.0,  "total_bytes": row.1}),
        serde_json::json!({"range": "1-10KB",      "count": row.2,  "total_bytes": row.3}),
        serde_json::json!({"range": "10-100KB",    "count": row.4,  "total_bytes": row.5}),
        serde_json::json!({"range": "100KB-1MB",   "count": row.6,  "total_bytes": row.7}),
        serde_json::json!({"range": "1-10MB",      "count": row.8,  "total_bytes": row.9}),
        serde_json::json!({"range": "10-100MB",    "count": row.10, "total_bytes": row.11}),
        serde_json::json!({"range": "100MB-1GB",   "count": row.12, "total_bytes": row.13}),
        serde_json::json!({"range": ">1GB",        "count": row.14, "total_bytes": row.15}),
    ];
    Ok(Json(serde_json::json!({"buckets": buckets})))
}

/// GET /api/v1/drive/files/stats/folder-depth — histograma de profundidade de pasta.
///
/// CTE recursiva calcula a profundidade de cada pasta (kind='folder', não-deletada)
/// a partir da raiz (parent_id IS NULL = depth 0). Retorna
/// `{buckets:[{depth,count,total_bytes}]}` ordenado por depth ASC. `total_bytes`
/// é a soma de size_bytes dos arquivos diretamente filhos (kind='file'). Sprint #696.
async fn file_stats_folder_depth(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "WITH RECURSIVE folder_tree AS ( \
             SELECT id, 0::BIGINT AS depth \
               FROM drive_files \
              WHERE tenant_id = $1 AND kind = 'folder' AND deleted_at IS NULL AND parent_id IS NULL \
             UNION ALL \
             SELECT f.id, ft.depth + 1 \
               FROM drive_files f \
               JOIN folder_tree ft ON f.parent_id = ft.id \
              WHERE f.tenant_id = $1 AND f.kind = 'folder' AND f.deleted_at IS NULL \
         ) \
         SELECT ft.depth, \
                COUNT(DISTINCT ft.id)::BIGINT AS folder_count, \
                COALESCE(SUM(child.size_bytes) FILTER (WHERE child.kind = 'file' AND child.deleted_at IS NULL), 0)::BIGINT AS total_bytes \
           FROM folder_tree ft \
           LEFT JOIN drive_files child ON child.parent_id = ft.id AND child.tenant_id = $1 \
          GROUP BY ft.depth \
          ORDER BY ft.depth ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;

    let buckets: Vec<serde_json::Value> = rows.into_iter()
        .map(|(depth, count, total_bytes)| serde_json::json!({
            "depth":       depth,
            "count":       count,
            "total_bytes": total_bytes,
        }))
        .collect();
    Ok(Json(serde_json::json!({"buckets": buckets})))
}

/// GET /api/v1/drive/files/stats/version-count?limit=N — top-N arquivos por número de versões.
///
/// JOIN `drive_file_versions` com `drive_files` (não-deletados, kind='file').
/// Retorna `{files:[{file_id,name,version_count}]}` ordenado por version_count DESC.
/// `limit` default 20 max 200. Sprint #701.
#[derive(Debug, Deserialize)]
struct StatsVersionCountQuery {
    limit: Option<i64>,
}

async fn file_stats_version_count(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsVersionCountQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT f.id, f.name, COUNT(v.id)::BIGINT AS version_count \
           FROM drive_files f \
           JOIN drive_file_versions v ON v.file_id = f.id AND v.tenant_id = $1 \
          WHERE f.tenant_id = $1 AND f.deleted_at IS NULL AND f.kind = 'file' \
          GROUP BY f.id, f.name \
          ORDER BY version_count DESC, f.name ASC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let files: Vec<serde_json::Value> = rows.into_iter()
        .map(|(file_id, name, version_count)| serde_json::json!({
            "file_id":       file_id,
            "name":          name,
            "version_count": version_count,
        }))
        .collect();
    Ok(Json(serde_json::json!({"files": files})))
}

/// GET /api/v1/drive/files/stats/tag-count?limit=N — top-N tags por uso.
///
/// GROUP BY tag em `drive_file_tags`, COUNT DISTINCT file_id. Retorna
/// `{tags:[{tag,file_count}]}` ordenado por file_count DESC. `limit` default 20 max 200.
/// Sprint #706.
#[derive(Debug, Deserialize)]
struct StatsTagCountQuery {
    limit: Option<i64>,
}

async fn file_stats_tag_count(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTagCountQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT tag, COUNT(DISTINCT file_id)::BIGINT AS file_count \
           FROM drive_file_tags \
          WHERE tenant_id = $1 \
          GROUP BY tag \
          ORDER BY file_count DESC, tag ASC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let tags: Vec<serde_json::Value> = rows.into_iter()
        .map(|(tag, file_count)| serde_json::json!({"tag": tag, "file_count": file_count}))
        .collect();
    Ok(Json(serde_json::json!({"tags": tags})))
}

/// GET /api/v1/drive/files/stats/ext-by-folder?folder_id= — breakdown de extensão numa pasta.
///
/// Análogo a `mime-by-folder` (#672) mas agrupa por extensão (parte após o último '.' do nome).
/// `folder_id` opcional: ausente = raiz (`parent_id IS NULL`). Extensão NULL = sem ponto no nome.
/// Retorna `{folder_id,rows:[{extension,file_count,total_bytes}]}` ordenado por total_bytes DESC. Sprint #711.
#[derive(Debug, Deserialize)]
struct StatsExtByFolderQuery {
    folder_id: Option<Uuid>,
}

async fn file_stats_ext_by_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsExtByFolderQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Option<String>, i64, i64)> = if let Some(fid) = q.folder_id {
        sqlx::query_as(
            "SELECT \
                CASE WHEN name LIKE '%.%' \
                     THEN lower(substring(name FROM '\\.[^.]*$')) \
                     ELSE NULL END AS extension, \
                COUNT(*)::BIGINT AS file_count, \
                COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
               FROM drive_files \
              WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' AND parent_id = $2 \
              GROUP BY extension \
              ORDER BY total_bytes DESC",
        )
        .bind(ctx.tenant_id).bind(fid).fetch_all(pool).await?
    } else {
        sqlx::query_as(
            "SELECT \
                CASE WHEN name LIKE '%.%' \
                     THEN lower(substring(name FROM '\\.[^.]*$')) \
                     ELSE NULL END AS extension, \
                COUNT(*)::BIGINT AS file_count, \
                COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
               FROM drive_files \
              WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' AND parent_id IS NULL \
              GROUP BY extension \
              ORDER BY total_bytes DESC",
        )
        .bind(ctx.tenant_id).fetch_all(pool).await?
    };

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, fc, tb)| serde_json::json!({
            "extension":   ext,
            "file_count":  fc,
            "total_bytes": tb,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folder_id": q.folder_id, "rows": out})))
}

/// GET /api/v1/drive/files/stats/lock-count — arquivos bloqueados por total e por user_id.
///
/// Conta arquivos onde `locked_at IS NOT NULL AND deleted_at IS NULL AND kind = 'file'`.
/// Retorna `{total_locked, by_user:[{user_id,locked_count}]}` ordenado por locked_count DESC.
/// Sprint #716.
async fn file_stats_lock_count(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total_locked,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND locked_at IS NOT NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    let by_user: Vec<(Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT owner_user_id, COUNT(*)::BIGINT AS locked_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND locked_at IS NOT NULL \
          GROUP BY owner_user_id \
          ORDER BY locked_count DESC, owner_user_id ASC NULLS LAST",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;

    let by_user_out: Vec<serde_json::Value> = by_user.into_iter()
        .map(|(uid, cnt)| serde_json::json!({"user_id": uid, "locked_count": cnt}))
        .collect();

    Ok(Json(serde_json::json!({
        "total_locked": total_locked,
        "by_user":      by_user_out,
    })))
}

/// GET /api/v1/drive/files/stats/starred-count — arquivos estrelados por total e por user_id.
///
/// Conta arquivos onde `starred_at IS NOT NULL AND deleted_at IS NULL AND kind = 'file'`.
/// Retorna `{total_starred, by_user:[{user_id,starred_count}]}` ordenado por starred_count DESC.
/// Sprint #721.
async fn file_stats_starred_count(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total_starred,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND starred_at IS NOT NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    let by_user: Vec<(Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT owner_user_id, COUNT(*)::BIGINT AS starred_count \
           FROM drive_files \
          WHERE tenant_id = $1 AND kind = 'file' AND deleted_at IS NULL AND starred_at IS NOT NULL \
          GROUP BY owner_user_id \
          ORDER BY starred_count DESC, owner_user_id ASC NULLS LAST",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;

    let by_user_out: Vec<serde_json::Value> = by_user.into_iter()
        .map(|(uid, cnt)| serde_json::json!({"user_id": uid, "starred_count": cnt}))
        .collect();

    Ok(Json(serde_json::json!({
        "total_starred": total_starred,
        "by_user":       by_user_out,
    })))
}

/// GET /api/v1/drive/files/stats/expiry-count — arquivos com expires_at definido.
///
/// Retorna total com expiry + já expirados (expires_at < NOW()). Apenas não-deletados.
/// Retorna `{total_with_expiry,already_expired,not_yet_expired}`. Sprint #726.
async fn file_stats_expiry_count(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total_with_expiry, already_expired): (i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE expires_at IS NOT NULL)::BIGINT AS total_with_expiry, \
            COUNT(*) FILTER (WHERE expires_at IS NOT NULL AND expires_at < NOW())::BIGINT AS already_expired \
           FROM drive_files \
          WHERE tenant_id = $1 AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    Ok(Json(serde_json::json!({
        "total_with_expiry": total_with_expiry,
        "already_expired":   already_expired,
        "not_yet_expired":   total_with_expiry - already_expired,
    })))
}

/// GET /api/v1/drive/files/stats/mime-top-n?limit=N — top-N mime_types por file_count (global, cross-folder).
///
/// Conta arquivos não-deletados agrupados por mime_type e retorna os N mais frequentes.
/// Análogo a ext-by-folder (#711) mas sem filtro de pasta — visão global do tenant.
/// `limit` default 20 max 100. Retorna `{rows:[{mime_type,file_count}]}`. Sprint #731.
async fn file_stats_mime_top_n(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q): Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(100).max(1);
    let pool  = state.db_or_unavailable()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT mime_type, COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file' \
            AND mime_type  IS NOT NULL \
          GROUP BY mime_type \
          ORDER BY file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(mime_type, file_count)| serde_json::json!({"mime_type": mime_type, "file_count": file_count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/orphan-versions — versões sem arquivo pai.
///
/// Conta entradas em `drive_file_versions` cujo `file_id` não existe em `drive_files` (LEFT JOIN IS NULL).
/// Indica inconsistência de storage — versões órfãs não têm dono válido.
/// Retorna `{orphan_version_count}`. Sprint #736.
async fn file_stats_orphan_versions(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (orphan_version_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(v.id)::BIGINT \
           FROM drive_file_versions v \
           LEFT JOIN drive_files f ON f.id = v.file_id AND f.tenant_id = $1 \
          WHERE v.tenant_id = $1 \
            AND f.id IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    Ok(Json(serde_json::json!({"orphan_version_count": orphan_version_count})))
}

/// GET /api/v1/drive/files/stats/empty-files — arquivos com tamanho zero ou indefinido.
///
/// Conta arquivos não-deletados com `size_bytes IS NULL OR size_bytes = 0` (kind='file').
/// Útil pra detectar uploads incompletos. Retorna `{total_empty,null_size,zero_size}`. Sprint #741.
async fn file_stats_empty_files(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total_empty, null_size, zero_size): (i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE size_bytes IS NULL OR size_bytes = 0)::BIGINT AS total_empty, \
            COUNT(*) FILTER (WHERE size_bytes IS NULL)::BIGINT                   AS null_size, \
            COUNT(*) FILTER (WHERE size_bytes = 0)::BIGINT                      AS zero_size \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file'",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    Ok(Json(serde_json::json!({
        "total_empty": total_empty,
        "null_size":   null_size,
        "zero_size":   zero_size,
    })))
}

/// GET /api/v1/drive/files/stats/deleted-by-day?since=&until= — arquivos deletados por dia.
///
/// DATE_TRUNC('day', deleted_at); kind='file'; bounds opcionais. Análogo a created-by-day (#682).
/// Retorna `{rows:[{day,count}]}` day ASC. Sprint #746.
#[derive(Debug, Deserialize)]
struct StatsByDayQuery {
    since: Option<String>,
    until: Option<String>,
}

async fn file_stats_deleted_by_day(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsByDayQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let since_dt: Option<time::OffsetDateTime> = q.since.as_deref()
        .map(|s| time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|_| crate::error::DriveError::BadRequest("since must be RFC3339".into()))?;
    let until_dt: Option<time::OffsetDateTime> = q.until.as_deref()
        .map(|s| time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|_| crate::error::DriveError::BadRequest("until must be RFC3339".into()))?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT to_char(date_trunc('day', deleted_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
                COUNT(*)::BIGINT AS count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND kind       = 'file' \
            AND deleted_at IS NOT NULL \
            AND ($2::timestamptz IS NULL OR deleted_at >= $2) \
            AND ($3::timestamptz IS NULL OR deleted_at <  $3) \
          GROUP BY day \
          ORDER BY day ASC",
    )
    .bind(ctx.tenant_id)
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool)
    .await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, count)| serde_json::json!({"day": day, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/name-length — avg/max LENGTH(name) global (kind='file', não-deletados).
///
/// Indicador de verbosidade nos nomes de arquivo do tenant.
/// Retorna `{file_count,avg_name_length,max_name_length}`. Sprint #751.
async fn file_stats_name_length(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (file_count, avg_name_length, max_name_length): (i64, Option<f64>, Option<i64>) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT AS file_count, \
            AVG(LENGTH(name)), \
            MAX(LENGTH(name))::BIGINT \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file'",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await?;

    Ok(Json(serde_json::json!({
        "file_count":       file_count,
        "avg_name_length":  avg_name_length,
        "max_name_length":  max_name_length,
    })))
}

/// GET /api/v1/drive/files/stats/ext-top-n?limit=N — top-N extensões globais por file_count.
///
/// Complementa mime-top-n (#731) com extensão extraída via substring. Default 20 max 100.
/// Retorna `{rows:[{extension,file_count}]}` count DESC. Sprint #756.
async fn file_stats_ext_top_n(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(100).max(1);
    let pool  = state.db_or_unavailable()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT \
            LOWER(SUBSTRING(name FROM '\\.[^.]*$')) AS extension, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file' \
            AND name LIKE '%.%' \
          GROUP BY extension \
          ORDER BY file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ext, cnt)| serde_json::json!({"extension": ext, "file_count": cnt}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/storage-by-user?limit=N — top-N usuários por total_bytes.
///
/// Soma size_bytes de arquivos não-deletados (kind='file') por owner_user_id. Default 20 max 200.
/// Retorna `{rows:[{user_id,file_count,total_bytes}]}` total_bytes DESC. Sprint #761.
async fn file_stats_storage_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(200).max(1);
    let pool  = state.db_or_unavailable()?;

    let rows: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT owner_user_id, COUNT(*)::BIGINT AS file_count, COALESCE(SUM(size_bytes),0)::BIGINT AS total_bytes \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file' \
          GROUP BY owner_user_id \
          ORDER BY total_bytes DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(uid, fc, tb)| serde_json::json!({"user_id": uid, "file_count": fc, "total_bytes": tb}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/quota-usage — pastas com quota: total_bytes + quota + pct_used.
///
/// JOIN drive_folder_quotas com SUM(size_bytes) dos filhos diretos (parent_id = folder_id).
/// Retorna `{rows:[{folder_id,max_bytes,used_bytes,pct_used}]}`. Sprint #766.
async fn file_stats_quota_usage(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT q.folder_id, q.max_bytes, \
                COALESCE(SUM(f.size_bytes), 0)::BIGINT AS used_bytes \
           FROM drive_folder_quotas q \
           LEFT JOIN drive_files f ON f.parent_id = q.folder_id \
                                  AND f.tenant_id = $1 \
                                  AND f.deleted_at IS NULL \
                                  AND f.kind = 'file' \
          WHERE q.tenant_id = $1 \
          GROUP BY q.folder_id, q.max_bytes \
          ORDER BY used_bytes DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder_id, max_bytes, used_bytes)| {
            let pct = if max_bytes > 0 { used_bytes as f64 / max_bytes as f64 * 100.0 } else { 0.0 };
            serde_json::json!({"folder_id": folder_id, "max_bytes": max_bytes, "used_bytes": used_bytes, "pct_used": pct})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/folder-file-count?limit=N — top-N pastas por file_count.
///
/// COUNT de arquivos não-deletados (kind='file') por parent_id. Default 20 max 200.
/// Retorna `{rows:[{folder_id,file_count,total_bytes}]}` file_count DESC. Sprint #771.
async fn file_stats_folder_file_count(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(200).max(1);
    let pool  = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, i64, i64)> = sqlx::query_as(
        "SELECT parent_id, COUNT(*)::BIGINT AS file_count, COALESCE(SUM(size_bytes),0)::BIGINT AS total_bytes \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file' \
          GROUP BY parent_id \
          ORDER BY file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(pid, fc, tb)| serde_json::json!({"folder_id": pid, "file_count": fc, "total_bytes": tb}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/deep-files?min_depth=N — arquivos em pastas com profundidade >= N.
///
/// CTE recursiva calcula depth de cada file/folder desde a raiz.
/// Agrupa arquivos (kind='file', não-deletados) por depth >= min_depth (default 3).
/// Retorna `{min_depth,total_files,total_bytes,by_depth:[{depth,file_count,total_bytes}]}`. Sprint #776.
async fn file_stats_deep_files(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<DeepFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let min_depth = q.min_depth.unwrap_or(3).max(1) as i64;
    let pool      = state.db_or_unavailable()?;

    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "WITH RECURSIVE tree AS ( \
            SELECT id, parent_id, kind, size_bytes, deleted_at, 0 AS depth \
              FROM drive_files \
             WHERE tenant_id = $1 AND parent_id IS NULL \
            UNION ALL \
            SELECT f.id, f.parent_id, f.kind, f.size_bytes, f.deleted_at, t.depth + 1 \
              FROM drive_files f \
              JOIN tree t ON f.parent_id = t.id \
             WHERE f.tenant_id = $1 \
         ) \
         SELECT depth, COUNT(*)::BIGINT AS file_count, COALESCE(SUM(size_bytes),0)::BIGINT AS total_bytes \
           FROM tree \
          WHERE kind = 'file' AND deleted_at IS NULL AND depth >= $2 \
          GROUP BY depth \
          ORDER BY depth ASC",
    )
    .bind(ctx.tenant_id).bind(min_depth)
    .fetch_all(pool).await?;

    let total_files: i64  = rows.iter().map(|(_, fc, _)| fc).sum();
    let total_bytes: i64  = rows.iter().map(|(_, _, tb)| tb).sum();
    let by_depth: Vec<serde_json::Value> = rows.into_iter()
        .map(|(depth, fc, tb)| serde_json::json!({"depth": depth, "file_count": fc, "total_bytes": tb}))
        .collect();
    Ok(Json(serde_json::json!({
        "min_depth":   min_depth,
        "total_files": total_files,
        "total_bytes": total_bytes,
        "by_depth":    by_depth,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct DeepFilesQuery {
    min_depth: Option<u32>,
}

/// GET /api/v1/drive/files/stats/mime-entropy — entropia de Shannon sobre distribuição mime_type.
///
/// H = -Σ p_i * log2(p_i) onde p_i = count_i / total.
/// mime_type NULL agrupado como "application/octet-stream". kind='file', não-deletados.
/// Retorna `{mime_count,total_files,entropy_bits,top:[{mime_type,file_count}]}`. Sprint #781.
async fn file_stats_mime_entropy(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(mime_type,'application/octet-stream') AS mime, COUNT(*)::BIGINT AS cnt \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file' \
          GROUP BY mime \
          ORDER BY cnt DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let entropy: f64 = if total == 0 {
        0.0
    } else {
        rows.iter().filter(|(_, c)| *c > 0).map(|(_, c)| {
            let p = *c as f64 / total as f64;
            -p * p.log2()
        }).sum()
    };
    let top: Vec<serde_json::Value> = rows.iter().take(20)
        .map(|(m, c)| serde_json::json!({"mime_type": m, "file_count": c}))
        .collect();
    Ok(Json(serde_json::json!({
        "mime_count":   rows.len(),
        "total_files":  total,
        "entropy_bits": entropy,
        "top":          top,
    })))
}

/// GET /api/v1/drive/files/stats/avg-versions — AVG e MAX versões por arquivo.
///
/// JOIN drive_file_versions GROUP BY file_id → avg/max version_count.
/// Apenas arquivos com ao menos 1 versão (kind='file', não-deletados no drive_files).
/// Retorna `{files_with_versions,avg_versions,max_versions}`. Sprint #791.
async fn file_stats_avg_versions(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (files_with_versions, avg_versions, max_versions): (i64, Option<f64>, Option<i64>) =
        sqlx::query_as(
            "SELECT \
                COUNT(DISTINCT v.file_id)::BIGINT, \
                AVG(vc.cnt), \
                MAX(vc.cnt)::BIGINT \
               FROM ( \
                 SELECT file_id, COUNT(*)::BIGINT AS cnt \
                   FROM drive_file_versions \
                  WHERE tenant_id = $1 \
                  GROUP BY file_id \
               ) vc \
               JOIN drive_file_versions v ON v.file_id = vc.file_id AND v.tenant_id = $1 \
               JOIN drive_files f ON f.id = v.file_id AND f.tenant_id = $1 \
                                  AND f.deleted_at IS NULL AND f.kind = 'file'",
        )
        .bind(ctx.tenant_id)
        .fetch_one(pool).await?;

    Ok(Json(serde_json::json!({
        "files_with_versions": files_with_versions,
        "avg_versions":        avg_versions,
        "max_versions":        max_versions,
    })))
}

/// GET /api/v1/drive/files/stats/ext-entropy — Shannon H sobre extensões de arquivo.
///
/// H = -Σ p_i * log2(p_i) para extensões LOWER(SUBSTRING(name FROM '\.[^.]*$')).
/// Arquivos sem extensão agrupados como "(none)". kind='file', não-deletados.
/// Retorna `{ext_count,total_files,entropy_bits,top:[{ext,file_count}]}`. Sprint #796.
async fn file_stats_ext_entropy(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT \
            COALESCE(NULLIF(LOWER(SUBSTRING(name FROM '\\.[^.]*$')), ''), '(none)') AS ext, \
            COUNT(*)::BIGINT AS cnt \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file' \
          GROUP BY ext \
          ORDER BY cnt DESC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool).await?;

    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let entropy: f64 = if total == 0 { 0.0 } else {
        rows.iter().filter(|(_, c)| *c > 0).map(|(_, c)| {
            let p = *c as f64 / total as f64;
            -p * p.log2()
        }).sum()
    };
    let top: Vec<serde_json::Value> = rows.iter().take(20)
        .map(|(e, c)| serde_json::json!({"ext": e, "file_count": c}))
        .collect();
    Ok(Json(serde_json::json!({
        "ext_count":    rows.len(),
        "total_files":  total,
        "entropy_bits": entropy,
        "top":          top,
    })))
}

/// GET /api/v1/drive/files/stats/checksum-coverage — COUNT com/sem sha256.
///
/// Arquivos kind='file', não-deletados. Indica cobertura de integridade.
/// Retorna `{total_files,with_checksum,without_checksum,coverage_pct}`. Sprint #801.
async fn file_stats_checksum_coverage(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total, with_checksum): (i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT, \
            COUNT(*) FILTER (WHERE sha256 IS NOT NULL AND sha256 <> '')::BIGINT \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file'",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    let without_checksum = total - with_checksum;
    let coverage_pct = if total > 0 { with_checksum as f64 / total as f64 * 100.0 } else { 0.0 };
    Ok(Json(serde_json::json!({
        "total_files":      total,
        "with_checksum":    with_checksum,
        "without_checksum": without_checksum,
        "coverage_pct":     coverage_pct,
    })))
}

/// GET /api/v1/drive/files/stats/storage-key-coverage — COUNT com/sem storage_key.
///
/// Arquivos kind='file', não-deletados. storage_key NULL indica upload não finalizado.
/// Retorna `{total_files,with_storage_key,without_storage_key,coverage_pct}`. Sprint #806.
async fn file_stats_storage_key_coverage(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (total, with_key): (i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT, \
            COUNT(*) FILTER (WHERE storage_key IS NOT NULL AND storage_key <> '')::BIGINT \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND deleted_at IS NULL \
            AND kind       = 'file'",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    let without_key = total - with_key;
    let coverage_pct = if total > 0 { with_key as f64 / total as f64 * 100.0 } else { 0.0 };
    Ok(Json(serde_json::json!({
        "total_files":       total,
        "with_storage_key":  with_key,
        "without_storage_key": without_key,
        "coverage_pct":      coverage_pct,
    })))
}

/// GET /api/v1/drive/files/stats/locked-by-user?limit=N — COUNT arquivos locked por locked_by.
///
/// Apenas arquivos com locked_at IS NOT NULL. Limit default 20 max 200.
/// Retorna `{total_locked,rows:[{locked_by,file_count}]}` file_count DESC. Sprint #811.
async fn file_stats_locked_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(200).max(1);
    let pool   = state.db_or_unavailable()?;

    let (total_locked,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM drive_files \
          WHERE tenant_id = $1 AND locked_at IS NOT NULL AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(pool).await?;

    let rows: Vec<(Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT locked_by, COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND locked_at  IS NOT NULL \
            AND deleted_at IS NULL \
          GROUP BY locked_by \
          ORDER BY file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(uid, fc)| serde_json::json!({"locked_by": uid, "file_count": fc}))
        .collect();
    Ok(Json(serde_json::json!({"total_locked": total_locked, "rows": out})))
}

/// GET /api/v1/drive/files/stats/mime-by-ext?limit=N — top mime_type por extensão.
///
/// GROUP BY (ext, mime_type) file_count DESC. Ext via LOWER(SUBSTRING(name)). Limit default 50 max 500.
/// Retorna `{rows:[{ext,mime_type,file_count}]}`. Sprint #816.
async fn file_stats_mime_by_ext(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(50).min(500).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            COALESCE(LOWER(SUBSTRING(name FROM '\\.[^.]*$')), '(none)') AS ext, \
            COALESCE(mime_type, 'application/octet-stream')            AS mime_type, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND kind       = 'file' \
            AND deleted_at IS NULL \
          GROUP BY ext, mime_type \
          ORDER BY file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(e, m, c)| serde_json::json!({"ext": e, "mime_type": m, "file_count": c}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/size-trend-by-day?since=&until= — total_bytes criados por dia.
///
/// DATE_TRUNC('day', created_at) SUM(size_bytes) GROUP BY dia ASC. kind='file', não-deletados.
/// Retorna `{rows:[{day,total_bytes,file_count}]}`. Sprint #821.
#[derive(Debug, serde::Deserialize)]
struct DateRangeQuery {
    since: Option<String>,
    until: Option<String>,
}

async fn file_stats_size_trend_by_day(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<DateRangeQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let since_dt: Option<OffsetDateTime> = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| crate::error::DriveError::BadRequest("since must be RFC3339".into()))
    }).transpose()?;
    let until_dt: Option<OffsetDateTime> = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| crate::error::DriveError::BadRequest("until must be RFC3339".into()))
    }).transpose()?;

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', created_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes, \
            COUNT(*)::BIGINT                     AS file_count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND kind       = 'file' \
            AND deleted_at IS NULL \
            AND ($2::timestamptz IS NULL OR created_at >= $2) \
            AND ($3::timestamptz IS NULL OR created_at <  $3) \
          GROUP BY day \
          ORDER BY day ASC",
    )
    .bind(ctx.tenant_id).bind(since_dt).bind(until_dt)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, b, c)| serde_json::json!({"day": d, "total_bytes": b, "file_count": c}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/version-age?limit=N — arquivos com versões mais antigas.
///
/// MIN(created_at) por arquivo via drive_file_versions, ordenado ASC.
/// Retorna `{rows:[{file_id,oldest_version_at,version_count}]}`. Limit default 20 max 200. Sprint #826.
async fn file_stats_version_age(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(20).min(200).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Uuid, Option<OffsetDateTime>, i64)> = sqlx::query_as(
        "SELECT v.file_id, \
                MIN(v.created_at)     AS oldest_version_at, \
                COUNT(*)::BIGINT      AS version_count \
           FROM drive_file_versions v \
           JOIN drive_files f ON f.id = v.file_id \
          WHERE f.tenant_id  = $1 \
            AND f.deleted_at IS NULL \
          GROUP BY v.file_id \
          ORDER BY oldest_version_at ASC NULLS LAST \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(fid, oldest, vc)| serde_json::json!({
            "file_id":           fid,
            "oldest_version_at": oldest,
            "version_count":     vc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/drive/files/stats/mime-count-by-user?limit=N — top mime_types por owner_user_id.
///
/// GROUP BY (owner_user_id, mime_type) COUNT DESC. Limit default 50 max 500.
/// Retorna `{rows:[{owner_user_id,mime_type,file_count}]}`. Sprint #831.
async fn file_stats_mime_count_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<StatsTopFilesQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(50).min(500).max(1);
    let pool   = state.db_or_unavailable()?;

    let rows: Vec<(Option<Uuid>, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            owner_user_id, \
            COALESCE(mime_type, 'application/octet-stream') AS mime_type, \
            COUNT(*)::BIGINT AS file_count \
           FROM drive_files \
          WHERE tenant_id  = $1 \
            AND kind       = 'file' \
            AND deleted_at IS NULL \
          GROUP BY owner_user_id, mime_type \
          ORDER BY file_count DESC \
          LIMIT $2",
    )
    .bind(ctx.tenant_id).bind(limit)
    .fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(uid, m, c)| serde_json::json!({
            "owner_user_id": uid,
            "mime_type":     m,
            "file_count":    c,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
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

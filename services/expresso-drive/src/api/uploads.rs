//! Drive resumable uploads — tus.io protocol v1.0.0.
//!
//! Spec: https://tus.io/protocols/resumable-upload
//!
//! Endpoints:
//! - POST   /api/v1/drive/uploads               → criação (Upload-Length + Upload-Metadata)
//! - HEAD   /api/v1/drive/uploads/:id           → status (retorna Upload-Offset/Upload-Length)
//! - PATCH  /api/v1/drive/uploads/:id           → envio de chunk (Content-Type: application/offset+octet-stream)
//! - DELETE /api/v1/drive/uploads/:id           → abort
//! - OPTIONS /api/v1/drive/uploads              → discovery
//!
//! Extensions suportadas: creation, termination. (concatenation/expiration não.)

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header::HeaderName, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{head as head_route, post as post_route},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::{fs, io::{AsyncSeekExt, AsyncWriteExt}};
use uuid::Uuid;

use crate::{
    api::context::RequestCtx,
    domain::{FileRepo, NewFile, NewUpload, NewVersion, QuotaRepo, UploadRepo, VersionRepo},
    error::{DriveError, Result},
    state::AppState,
};

const TUS_VERSION:      &str = "1.0.0";
const TUS_SUPPORTED:    &str = "1.0.0";
const TUS_EXTENSIONS:   &str = "creation,termination";
const MAX_UPLOAD_BYTES: i64  = 50 * 1024 * 1024 * 1024;   // 50 GB hard cap.

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/drive/uploads",
            post_route(create_upload_h).options(options_collection))
        .route("/api/v1/drive/uploads/:id",
            head_route(head_upload_h).patch(patch_upload_h).delete(delete_upload_h))
}

fn tus_headers() -> Vec<(HeaderName, HeaderValue)> {
    vec![
        (HeaderName::from_static("tus-resumable"), HeaderValue::from_static(TUS_VERSION)),
        (HeaderName::from_static("tus-version"),   HeaderValue::from_static(TUS_SUPPORTED)),
        (HeaderName::from_static("tus-extension"), HeaderValue::from_static(TUS_EXTENSIONS)),
    ]
}

fn parse_metadata(h: Option<&HeaderValue>) -> (Option<String>, Option<String>) {
    // "filename base64name,filetype base64mime,parent base64uuid"
    let Some(v) = h.and_then(|x| x.to_str().ok()) else { return (None, None); };
    let mut name  = None;
    let mut mime  = None;
    for part in v.split(',') {
        let mut it = part.trim().splitn(2, ' ');
        let (k, val) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
        let decoded = STANDARD.decode(val.trim())
            .ok().and_then(|b| String::from_utf8(b).ok());
        match k {
            "filename" | "name"     => name = decoded,
            "filetype" | "mimetype" => mime = decoded,
            _ => {}
        }
    }
    (name, mime)
}

fn header_i64(h: &HeaderMap, name: &str) -> Option<i64> {
    h.get(name)?.to_str().ok()?.parse().ok()
}

async fn options_collection() -> Response {
    let mut h = HeaderMap::new();
    for (k, v) in tus_headers() { h.insert(k, v); }
    (StatusCode::NO_CONTENT, h).into_response()
}

async fn create_upload_h(
    State(st): State<AppState>, ctx: RequestCtx, headers: HeaderMap,
) -> Response {
    create_upload(st, ctx, headers).await.unwrap_or_else(|e| e.into_response())
}

async fn head_upload_h(
    State(st): State<AppState>, ctx: RequestCtx, Path(id): Path<Uuid>,
) -> Response { head_upload(st, ctx, id).await.unwrap_or_else(|e| e.into_response()) }

async fn patch_upload_h(
    State(st): State<AppState>, ctx: RequestCtx, Path(id): Path<Uuid>,
    headers: HeaderMap, body: Bytes,
) -> Response { patch_upload(st, ctx, id, headers, body).await.unwrap_or_else(|e| e.into_response()) }

async fn delete_upload_h(
    State(st): State<AppState>, ctx: RequestCtx, Path(id): Path<Uuid>,
) -> Response { delete_upload(st, ctx, id).await.unwrap_or_else(|e| e.into_response()) }

// ─── POST /uploads ───────────────────────────────────────────────────────────

async fn create_upload(
    st:      AppState,
    ctx:     RequestCtx,
    headers: HeaderMap,
) -> Result<Response> {
    let total = header_i64(&headers, "upload-length")
        .ok_or_else(|| DriveError::BadRequest("Upload-Length required".into()))?;
    if total < 0 || total > MAX_UPLOAD_BYTES {
        return Err(DriveError::BadRequest(format!("Upload-Length out of bounds (max {MAX_UPLOAD_BYTES})")));
    }

    let pool  = st.db_or_unavailable()?;
    let quota = QuotaRepo::new(pool).get(ctx.tenant_id).await?;
    if !quota.fits(total) {
        return Err(DriveError::QuotaExceeded);
    }

    let (name_md, mime_md) = parse_metadata(headers.get("upload-metadata"));
    let name = name_md.ok_or_else(|| DriveError::BadRequest("Upload-Metadata filename required".into()))?;
    let fname = sanitize_name(&name)?;

    let parent_id = headers.get("upload-parent-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| DriveError::BadRequest("invalid Upload-Parent-Id".into()))?;

    let key = Uuid::new_v4().to_string();
    let root = st.data_root();
    fs::create_dir_all(root).await?;
    let path: PathBuf = root.join(format!("{key}.part"));
    fs::File::create(&path).await?;   // reserva arquivo vazio.

    let repo = UploadRepo::new(pool);
    let up = repo.insert(&NewUpload {
        tenant_id:     ctx.tenant_id,
        owner_user_id: ctx.user_id,
        parent_id,
        name:          &fname,
        mime_type:     mime_md.as_deref(),
        total_size:    total,
        storage_key:   &key,
    }).await?;

    let mut h = HeaderMap::new();
    for (k, v) in tus_headers() { h.insert(k, v); }
    h.insert("location", format!("/api/v1/drive/uploads/{}", up.id).parse().unwrap());
    h.insert("upload-offset", "0".parse().unwrap());
    h.insert("upload-length", total.to_string().parse().unwrap());
    tracing::info!(target: "audit",
        event = "drive.upload.create",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        upload_id = %up.id, total_size = total);
    Ok((StatusCode::CREATED, h).into_response())
}

// ─── HEAD /uploads/:id ───────────────────────────────────────────────────────

async fn head_upload(st: AppState, ctx: RequestCtx, id: Uuid) -> Result<Response> {
    let pool = st.db_or_unavailable()?;
    let up = UploadRepo::new(pool).get(ctx.tenant_id, id).await?
        .ok_or(DriveError::NotFound(id))?;
    let mut h = HeaderMap::new();
    for (k, v) in tus_headers() { h.insert(k, v); }
    h.insert("upload-offset", up.offset_bytes.to_string().parse().unwrap());
    h.insert("upload-length", up.total_size.to_string().parse().unwrap());
    h.insert("cache-control", "no-store".parse().unwrap());
    Ok((StatusCode::OK, h).into_response())
}

// ─── PATCH /uploads/:id ──────────────────────────────────────────────────────

async fn patch_upload(
    st:      AppState,
    ctx:     RequestCtx,
    id:      Uuid,
    headers: HeaderMap,
    body:    Bytes,
) -> Result<Response> {
    let ct = headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !ct.starts_with("application/offset+octet-stream") {
        return Err(DriveError::BadRequest(
            "Content-Type must be application/offset+octet-stream".into()
        ));
    }
    let client_offset = header_i64(&headers, "upload-offset")
        .ok_or_else(|| DriveError::BadRequest("Upload-Offset required".into()))?;

    let pool = st.db_or_unavailable()?;
    let repo = UploadRepo::new(pool);
    let up = repo.get(ctx.tenant_id, id).await?
        .ok_or(DriveError::NotFound(id))?;

    if client_offset != up.offset_bytes {
        return Err(DriveError::Conflict(format!(
            "offset mismatch: expected {}, got {}", up.offset_bytes, client_offset
        )));
    }

    let incoming = body.len() as i64;
    let new_offset = up.offset_bytes + incoming;
    if new_offset > up.total_size {
        return Err(DriveError::BadRequest("chunk exceeds Upload-Length".into()));
    }

    // Append no .part file.
    let path: PathBuf = st.data_root().join(format!("{}.part", up.storage_key));
    let mut f = fs::OpenOptions::new().write(true).open(&path).await?;
    f.seek(std::io::SeekFrom::Start(up.offset_bytes as u64)).await?;
    f.write_all(&body).await?;
    f.flush().await?;

    // Atualiza offset via CAS.
    let updated = repo.advance_offset(ctx.tenant_id, id, up.offset_bytes, new_offset).await?;
    if updated.is_none() {
        return Err(DriveError::Conflict("concurrent PATCH — retry HEAD first".into()));
    }

    // Completou? → promove para drive_files + remove upload session.
    if new_offset == up.total_size {
        finalize_upload(&st, &ctx, &up).await?;
    }

    let mut h = HeaderMap::new();
    for (k, v) in tus_headers() { h.insert(k, v); }
    h.insert("upload-offset", new_offset.to_string().parse().unwrap());
    Ok((StatusCode::NO_CONTENT, h).into_response())
}

// ─── DELETE /uploads/:id ─────────────────────────────────────────────────────

async fn delete_upload(st: AppState, ctx: RequestCtx, id: Uuid) -> Result<Response> {
    let pool = st.db_or_unavailable()?;
    let repo = UploadRepo::new(pool);
    let Some(up) = repo.get(ctx.tenant_id, id).await? else {
        return Err(DriveError::NotFound(id));
    };
    // Remove .part blob best-effort.
    let path: PathBuf = st.data_root().join(format!("{}.part", up.storage_key));
    let _ = fs::remove_file(&path).await;
    repo.delete(ctx.tenant_id, id).await?;

    let mut h = HeaderMap::new();
    for (k, v) in tus_headers() { h.insert(k, v); }
    tracing::info!(target: "audit",
        event = "drive.upload.abort",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, upload_id = %id);
    Ok((StatusCode::NO_CONTENT, h).into_response())
}

// ─── Finalize → promote to drive_files ───────────────────────────────────────

async fn finalize_upload(
    st:  &AppState,
    ctx: &RequestCtx,
    up:  &crate::domain::UploadSession,
) -> Result<()> {
    let pool     = st.db_or_unavailable()?;
    let file_repo = FileRepo::new(pool);
    let ver_repo  = VersionRepo::new(pool);

    // Lê blob completo p/ computar sha256 + renomeia.part → final key.
    let root = st.data_root();
    let src: PathBuf = root.join(format!("{}.part", up.storage_key));
    let bytes = fs::read(&src).await?;
    let sha = format!("{:x}", Sha256::digest(&bytes));

    let final_key = up.storage_key.clone();
    let dst: PathBuf = root.join(&final_key);
    fs::rename(&src, &dst).await?;

    // Colisão por nome no mesmo parent → arquiva versão atual → overwrite.
    if let Some(existing) = file_repo.find_by_name(ctx.tenant_id, up.parent_id, &up.name).await? {
        if existing.kind != "file" {
            let _ = fs::remove_file(&dst).await;
            return Err(DriveError::Conflict("folder name collision".into()));
        }
        if let Some(prev_key) = existing.storage_key.as_deref() {
            let next_no = ver_repo.next_no(existing.id).await?;
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
        file_repo.update_content(
            ctx.tenant_id, existing.id,
            &final_key, up.total_size,
            Some(&sha), up.mime_type.as_deref(),
        ).await?;
    } else {
        file_repo.insert(&NewFile {
            tenant_id:     ctx.tenant_id,
            owner_user_id: ctx.user_id,
            parent_id:     up.parent_id,
            name:          up.name.clone(),
            kind:          "file".into(),
            mime_type:     up.mime_type.clone(),
            size_bytes:    up.total_size,
            sha256:        Some(sha),
            storage_key:   Some(final_key),
        }).await?;
    }

    UploadRepo::new(pool).delete(ctx.tenant_id, up.id).await?;
    tracing::info!(target: "audit",
        event = "drive.upload.finalize",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        upload_id = %up.id, name = %up.name, size = up.total_size);
    Ok(())
}

// Replicado de files.rs (scope-local). Se virar padrão, extrair p/ util.
fn sanitize_name(raw: &str) -> Result<String> {
    let t = raw.trim();
    if t.is_empty() || t.contains('/') || t.contains('\\') || t.contains('\0') {
        return Err(DriveError::BadRequest("invalid filename".into()));
    }
    Ok(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn parse_upload_metadata_standard() {
        let h = HeaderValue::from_static("filename aGVsbG8udHh0,filetype dGV4dC9wbGFpbg==");
        let (n, m) = parse_metadata(Some(&h));
        assert_eq!(n.as_deref(), Some("hello.txt"));
        assert_eq!(m.as_deref(), Some("text/plain"));
    }

    #[test]
    fn parse_upload_metadata_missing_filetype() {
        let h = HeaderValue::from_static("filename ZG9jLm9kdA==");
        let (n, m) = parse_metadata(Some(&h));
        assert_eq!(n.as_deref(), Some("doc.odt"));
        assert_eq!(m, None);
    }

    #[test]
    fn parse_upload_metadata_empty() {
        let (n, m) = parse_metadata(None);
        assert_eq!(n, None);
        assert_eq!(m, None);
    }

    #[test]
    fn sanitize_rejects_slash() {
        assert!(sanitize_name("../etc/passwd").is_err());
        assert!(sanitize_name("a\\b").is_err());
        assert!(sanitize_name("").is_err());
        assert_eq!(sanitize_name("ok.txt").unwrap(), "ok.txt");
    }
}

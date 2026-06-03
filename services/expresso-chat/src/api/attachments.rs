//! Chat message attachments.
//!
//! POST /api/v1/channels/:id/attachments  — multipart upload; stores bytes in
//!                                           the Matrix media repo, posts an
//!                                           m.<file> message, indexes the row.
//! GET  /api/v1/channels/:id/attachments  — list a channel's attachments.
//!
//! The upload field must be named `file`. Bytes live in Matrix (mxc_uri); the
//! `chat_attachments` row is the expresso-side index. An upload fans out on the
//! SSE bus as a `message` event so connected clients render it live.

use axum::{
    body::Bytes,
    extract::{Multipart, Path, Query, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{
    Attachment, AttachmentKind, AttachmentRepo, ChannelRepo, NewAttachment, ReadMarkerRepo,
    MAX_ATTACHMENT_BYTES, MAX_FILENAME_BYTES,
};
use crate::error::{ChatError, Result};
use crate::events::ChatEvent;
use crate::state::AppState;

/// Default/cap for the list endpoint.
const DEFAULT_LIST_LIMIT: i64 = 50;
const MAX_LIST_LIMIT: i64 = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/channels/:id/attachments", post(upload).get(list))
        .route(
            "/api/v1/channels/:id/attachments/:att_id/download",
            axum::routing::get(download),
        )
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    DEFAULT_LIST_LIMIT
}

/// One uploaded file pulled from the multipart body.
struct UploadedFile {
    filename: String,
    content_type: String,
    bytes: Bytes,
}

/// Pull the `file` field (filename, content-type, bytes) from the multipart
/// body, enforcing size/filename caps. Rejects a missing or empty file.
async fn read_file_field(mut mp: Multipart) -> Result<UploadedFile> {
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| ChatError::BadRequest(e.to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field
            .file_name()
            .map(ToOwned::to_owned)
            .ok_or_else(|| ChatError::BadRequest("file field missing filename".into()))?;
        let content_type = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_owned();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| ChatError::BadRequest(e.to_string()))?;
        validate_upload(&filename, bytes.len())?;
        return Ok(UploadedFile {
            filename,
            content_type,
            bytes,
        });
    }
    Err(ChatError::BadRequest("missing `file` field".into()))
}

fn validate_upload(filename: &str, size: usize) -> Result<()> {
    if filename.trim().is_empty() {
        return Err(ChatError::BadRequest("filename required".into()));
    }
    if filename.len() > MAX_FILENAME_BYTES {
        return Err(ChatError::BadRequest(format!(
            "filename too long: {} bytes (max {})",
            filename.len(),
            MAX_FILENAME_BYTES
        )));
    }
    if size == 0 {
        return Err(ChatError::BadRequest("empty file".into()));
    }
    if size > MAX_ATTACHMENT_BYTES {
        return Err(ChatError::BadRequest(format!(
            "attachment too large: {} bytes (max {})",
            size, MAX_ATTACHMENT_BYTES
        )));
    }
    Ok(())
}

async fn upload(
    state: State<AppState>,
    ctx: RequestCtx,
    channel_id: Path<Uuid>,
    mp: Multipart,
) -> Result<(StatusCode, Json<Attachment>)> {
    let r = upload_inner(state, ctx, channel_id, mp).await;
    crate::metrics::record_result(crate::metrics::OP_ATTACHMENT_UPLOAD, &r);
    r
}

async fn upload_inner(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(channel_id): Path<Uuid>,
    mp: Multipart,
) -> Result<(StatusCode, Json<Attachment>)> {
    let file = read_file_field(mp).await?;
    let pool = state.db_or_unavailable()?;
    let matrix = state.matrix_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo
        .is_member(ctx.tenant_id, channel_id, ctx.user_id)
        .await?
    {
        return Err(ChatError::NotMember);
    }
    let ch = repo
        .get(ctx.tenant_id, channel_id)
        .await
        .map_err(|_| ChatError::ChannelNotFound(channel_id))?;

    let acting_as = matrix.mxid_for(ctx.user_id);
    let kind = AttachmentKind::from_content_type(&file.content_type);
    let size_bytes = file.bytes.len() as i64;

    let mxc_uri = matrix
        .upload_media(
            &acting_as,
            &file.filename,
            &file.content_type,
            file.bytes.to_vec(),
        )
        .await?;
    let event_id = matrix
        .send_file_message(
            &acting_as,
            &ch.matrix_room_id,
            &crate::matrix::FileMessage {
                msgtype: kind.msgtype(),
                filename: &file.filename,
                content_type: &file.content_type,
                mxc_uri: &mxc_uri,
                size_bytes,
            },
        )
        .await?;

    let stored = AttachmentRepo::new(pool)
        .insert(
            ctx.tenant_id,
            &NewAttachment {
                channel_id,
                user_id: ctx.user_id,
                event_id: &event_id,
                filename: &file.filename,
                content_type: &file.content_type,
                size_bytes,
                kind,
                mxc_uri: &mxc_uri,
            },
        )
        .await?;

    // Live-deliver as a message event (body = filename) + unread bookkeeping,
    // mirroring a text send. Best-effort.
    state.bus().publish(ChatEvent::message(
        ctx.tenant_id,
        channel_id,
        ctx.user_id,
        event_id,
        file.filename,
    ));
    let marks = ReadMarkerRepo::new(pool);
    if let Err(e) = marks.bump_activity(ctx.tenant_id, channel_id).await {
        tracing::warn!(error = %e, channel = %channel_id, "chat: bump_activity failed");
    }
    if let Err(e) = marks
        .mark_read(ctx.tenant_id, channel_id, ctx.user_id)
        .await
    {
        tracing::warn!(error = %e, channel = %channel_id, "chat: sender mark_read failed");
    }

    Ok((StatusCode::CREATED, Json(stored)))
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(channel_id): Path<Uuid>,
    Query(mut q): Query<ListQuery>,
) -> Result<Json<Vec<Attachment>>> {
    if q.limit <= 0 || q.limit > MAX_LIST_LIMIT {
        q.limit = q.limit.clamp(1, MAX_LIST_LIMIT);
    }
    let pool = state.db_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo
        .is_member(ctx.tenant_id, channel_id, ctx.user_id)
        .await?
    {
        return Err(ChatError::NotMember);
    }
    let rows = AttachmentRepo::new(pool)
        .list_for_channel(ctx.tenant_id, channel_id, q.limit)
        .await?;
    Ok(Json(rows))
}

/// GET /api/v1/channels/:id/attachments/:att_id/download — stream the bytes of a
/// channel attachment, resolving its `mxc_uri` from the Matrix media repo.
/// Member-gated; 404 when the attachment isn't in this channel.
async fn download(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((channel_id, att_id)): Path<(Uuid, Uuid)>,
) -> Result<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse;

    let pool = state.db_or_unavailable()?;
    if !ChannelRepo::new(pool)
        .is_member(ctx.tenant_id, channel_id, ctx.user_id)
        .await?
    {
        return Err(ChatError::NotMember);
    }
    let att = AttachmentRepo::new(pool)
        .get(ctx.tenant_id, channel_id, att_id)
        .await?
        .ok_or(ChatError::AttachmentNotFound(att_id))?;

    let matrix = state.matrix_or_unavailable()?;
    let acting_as = matrix.mxid_for(ctx.user_id);
    let (content_type, bytes) = matrix.download_media(&acting_as, &att.mxc_uri).await?;

    // Quote the filename and strip CR/LF to keep the header well-formed.
    let safe_name = att.filename.replace(['"', '\r', '\n'], "");
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{safe_name}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_upload_accepts_normal_file() {
        assert!(validate_upload("photo.jpg", 2048).is_ok());
    }

    #[test]
    fn validate_upload_rejects_empty_filename() {
        let err = format!("{:?}", validate_upload("  ", 10).unwrap_err());
        assert!(err.contains("filename required"), "got: {err}");
    }

    #[test]
    fn validate_upload_rejects_oversize_filename() {
        let big = "x".repeat(MAX_FILENAME_BYTES + 1);
        let err = format!("{:?}", validate_upload(&big, 10).unwrap_err());
        assert!(err.contains("filename too long"), "got: {err}");
    }

    #[test]
    fn validate_upload_rejects_empty_file() {
        let err = format!("{:?}", validate_upload("f.txt", 0).unwrap_err());
        assert!(err.contains("empty file"), "got: {err}");
    }

    #[test]
    fn validate_upload_rejects_oversize_file() {
        let err = format!(
            "{:?}",
            validate_upload("big.bin", MAX_ATTACHMENT_BYTES + 1).unwrap_err()
        );
        assert!(err.contains("too large"), "got: {err}");
    }

    #[test]
    fn validate_upload_accepts_boundary_size() {
        assert!(validate_upload("f.bin", MAX_ATTACHMENT_BYTES).is_ok());
    }

    #[test]
    fn list_query_default_limit() {
        let q: ListQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(q.limit, DEFAULT_LIST_LIMIT);
    }

    #[test]
    fn list_limit_clamp_floors_to_one() {
        let mut limit: i64 = 0;
        if limit <= 0 || limit > MAX_LIST_LIMIT {
            limit = limit.clamp(1, MAX_LIST_LIMIT);
        }
        assert_eq!(limit, 1);
    }

    #[test]
    fn list_limit_clamp_caps_at_max() {
        let mut limit: i64 = 9999;
        if limit <= 0 || limit > MAX_LIST_LIMIT {
            limit = limit.clamp(1, MAX_LIST_LIMIT);
        }
        assert_eq!(limit, MAX_LIST_LIMIT);
    }
}

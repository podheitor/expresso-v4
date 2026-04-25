//! Attachment list + download endpoints.
//! Reads raw .eml from body_path, parses MIME parts via mail-parser.
//!
//! Tenant scoping: `fetch_body_path` abre tx via `begin_tenant_tx` e junta
//! `messages`→`mailboxes` filtrando `tenant_id` + `user_id` — sem isso
//! qualquer usuário autenticado baixava attachments de qualquer tenant.

use axum::{
    Router,
    routing::get,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    Json,
};
use expresso_core::begin_tenant_tx;
use mail_parser::{MessageParser, MimeHeaders};
use serde::Serialize;
use uuid::Uuid;

use crate::{api::context::RequestCtx, error::{MailError, Result}, state::AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/messages/:id/attachments",        get(list_attachments))
        .route("/mail/messages/:id/attachments/:index", get(download_attachment))
}

// ─── DTOs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct AttachmentMeta {
    pub index: usize,
    pub filename: Option<String>,
    pub content_type: String,
    pub size: usize,
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Load raw .eml bytes from body_path (S3 or local FS)
async fn load_raw(state: &AppState, body_path: &str) -> Result<Vec<u8>> {
    if let Some(key) = body_path.strip_prefix("s3://") {
        // Strip bucket prefix: "bucket/raw/xxx.eml" → "raw/xxx.eml"
        let key = key.split_once('/').map(|(_, k)| k).unwrap_or(key);
        let store = state.store().ok_or_else(|| {
            MailError::InvalidMessage("S3 body_path but no object store configured".into())
        })?;
        return store.get(key).await.map_err(|e| {
            MailError::InvalidMessage(format!("S3 get failed: {e}"))
        });
    }
    tokio::fs::read(body_path)
        .await
        .map_err(|e| MailError::InvalidMessage(format!("failed to read raw message: {e}")))
}

/// Fetch body_path for message id from DB, scoped to tenant+user.
async fn fetch_body_path(state: &AppState, ctx: &RequestCtx, id: Uuid) -> Result<String> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let path: Option<String> = sqlx::query_scalar(
        r#"SELECT m.body_path
             FROM messages  m
             JOIN mailboxes mb ON mb.id = m.mailbox_id
            WHERE m.id         = $1
              AND m.tenant_id  = $2
              AND mb.tenant_id = $2
              AND mb.user_id   = $3"#,
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    path.ok_or(MailError::MessageNotFound(id))
}

/// Format content-type from ContentType struct
fn format_ct(ct: &mail_parser::ContentType) -> String {
    match &ct.c_subtype {
        Some(sub) => format!("{}/{}", ct.c_type, sub),
        None => ct.c_type.to_string(),
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// GET /api/v1/mail/messages/:id/attachments — list attachment metadata
async fn list_attachments(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<Vec<AttachmentMeta>>> {
    let body_path = fetch_body_path(&state, &ctx, id).await?;
    let raw = load_raw(&state, &body_path).await?;
    let msg = MessageParser::default()
        .parse(&raw)
        .ok_or_else(|| MailError::InvalidMessage("failed to parse MIME".into()))?;

    let attachments: Vec<AttachmentMeta> = msg
        .attachments()
        .enumerate()
        .map(|(i, part)| {
            let ct = part
                .content_type()
                .map(format_ct)
                .unwrap_or_else(|| "application/octet-stream".into());
            AttachmentMeta {
                index: i,
                filename: part.attachment_name().map(String::from),
                content_type: ct,
                size: part.len(),
            }
        })
        .collect();

    Ok(Json(attachments))
}

/// GET /api/v1/mail/messages/:id/attachments/:index — download binary
async fn download_attachment(
    State(state):      State<AppState>,
    ctx:               RequestCtx,
    Path((id, index)): Path<(Uuid, usize)>,
) -> Result<Response> {
    let body_path = fetch_body_path(&state, &ctx, id).await?;
    let raw = load_raw(&state, &body_path).await?;
    let msg = MessageParser::default()
        .parse(&raw)
        .ok_or_else(|| MailError::InvalidMessage("failed to parse MIME".into()))?;

    let part = msg
        .attachments()
        .nth(index)
        .ok_or_else(|| MailError::InvalidMessage(format!("attachment index {index} not found")))?;

    let ct = part
        .content_type()
        .map(format_ct)
        .unwrap_or_else(|| "application/octet-stream".into());

    let filename = part
        .attachment_name()
        .unwrap_or("attachment")
        .to_owned();

    let body = part.contents().to_vec();

    // Both `ct` (MIME from headers) and `filename` (attachment-name parameter)
    // are attacker-controlled — any inbound email can set them. Without
    // sanitization, a CR/LF in the filename forces axum to return 500 when
    // it tries to build the response (HeaderValue rejects the bad bytes), so
    // a malicious sender could brick attachment downloads in the recipient's
    // inbox. Build header-safe values here.
    let ct_safe = sanitize_header_token(&ct, "application/octet-stream");
    let cd_safe = build_content_disposition(&filename);

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, ct_safe),
            (header::CONTENT_DISPOSITION, cd_safe),
        ],
        body,
    )
        .into_response())
}

/// Replace bytes that are not safe for an HTTP header *value* (anything <0x20
/// except TAB, plus DEL) with `_`. Falls back to `default` when the result
/// is empty after sanitizing.
fn sanitize_header_token(raw: &str, default: &str) -> String {
    let cleaned: String = raw.chars().map(|c| {
        let b = c as u32;
        if b == 0x09 || (0x20..0x7f).contains(&b) { c } else { '_' }
    }).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() { default.to_string() } else { trimmed.to_string() }
}

fn build_content_disposition(name: &str) -> String {
    let ascii: String = name.chars().map(|c| {
        if c.is_ascii_graphic() && c != '"' && c != '\\' { c } else { '_' }
    }).collect();
    let ascii = if ascii.trim().is_empty() { "attachment".into() } else { ascii };
    let pct = percent_encode_filename(name);
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{pct}")
}

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

#[cfg(test)]
mod tests {
    use super::*;

    const MULTIPART_EML: &[u8] = b"From: sender@example.com\r\n\
To: recipient@example.com\r\n\
Subject: Test with attachment\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"boundary42\"\r\n\
\r\n\
--boundary42\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
Hello world\r\n\
--boundary42\r\n\
Content-Type: application/pdf; name=\"report.pdf\"\r\n\
Content-Disposition: attachment; filename=\"report.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQKMSAwIG9iago=\r\n\
--boundary42--\r\n";

    #[test]
    fn parse_attachment_metadata() {
        let msg = MessageParser::default().parse(MULTIPART_EML).unwrap();
        let atts: Vec<_> = msg
            .attachments()
            .enumerate()
            .map(|(i, part)| AttachmentMeta {
                index: i,
                filename: part.attachment_name().map(String::from),
                content_type: part
                    .content_type()
                    .map(format_ct)
                    .unwrap_or_else(|| "application/octet-stream".into()),
                size: part.len(),
            })
            .collect();

        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].filename.as_deref(), Some("report.pdf"));
        assert_eq!(atts[0].content_type, "application/pdf");
        assert!(atts[0].size > 0);
    }

    #[test]
    fn parse_no_attachments() {
        let plain = b"From: a@b.com\r\nSubject: plain\r\n\r\nJust text\r\n";
        let msg = MessageParser::default().parse(plain.as_slice()).unwrap();
        assert_eq!(msg.attachment_count(), 0);
    }
}

//! Attachment list + download endpoints.
//! Reads raw .eml from body_path, parses MIME parts via mail-parser.
//!
//! Tenant scoping: `fetch_body_path` abre tx via `begin_tenant_tx` e junta
//! `messages`→`mailboxes` filtrando `tenant_id` + `user_id` — sem isso
//! qualquer usuário autenticado baixava attachments de qualquer tenant.

use axum::{
    Router,
    routing::get,
    extract::{Path, Query, State},
    http::{StatusCode, header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
    Json,
};
use expresso_core::begin_tenant_tx;
use mail_parser::{MessageParser, MimeHeaders, PartType};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{api::context::RequestCtx, error::{MailError, Result}, state::AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/messages/:id/attachments",        get(list_attachments))
        .route("/mail/messages/:id/attachments/:index", get(download_attachment))
        .route("/mail/messages/:id/headers",            get(message_headers))
        .route("/mail/messages/:id/body",               get(message_body))
        .route("/mail/messages/:id/structure",          get(message_structure))
        .route("/mail/messages/:id/inline-images",            get(list_inline_images))
        .route("/mail/messages/:id/inline-images/:index",     get(download_inline_image))
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

/// Fetch body_path + size_bytes for message id from DB, scoped to tenant+user.
async fn fetch_message_meta(state: &AppState, ctx: &RequestCtx, id: Uuid) -> Result<(String, i32)> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row: Option<(String, i32)> = sqlx::query_as(
        r#"SELECT m.body_path, m.size_bytes
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

    row.ok_or(MailError::MessageNotFound(id))
}

/// Format content-type from ContentType struct
fn format_ct(ct: &mail_parser::ContentType) -> String {
    match &ct.c_subtype {
        Some(sub) => format!("{}/{}", ct.c_type, sub),
        None => ct.c_type.to_string(),
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// GET /api/v1/mail/messages/:id/attachments — list attachment metadata.
/// ETag = `"{size_bytes}-{id}"` (immutable after delivery, same as GET /raw).
async fn list_attachments(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let (body_path, size_bytes) = fetch_message_meta(&state, &ctx, id).await?;

    let etag = format!("\"{}-{}\"", size_bytes, id);
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }

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

    let mut resp = Json(attachments).into_response();
    resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    Ok(resp)
}

/// GET /api/v1/mail/messages/:id/attachments/:index — download binary
async fn download_attachment(
    State(state):      State<AppState>,
    ctx:               RequestCtx,
    Path((id, index)): Path<(Uuid, usize)>,
) -> Result<Response> {
    let (body_path, _) = fetch_message_meta(&state, &ctx, id).await?;
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

/// GET /api/v1/mail/messages/:id/body?format=text|html — body de uma mensagem.
///
/// Retorna `{message_id, format, body}` com o corpo extraído do .eml via mail-parser.
/// `format=text` → primeiro part text/plain; `format=html` → primeiro part text/html.
/// 404 se mensagem não pertence ao tenant/user. 400 se format inválido.
/// 404 com mensagem específica se o format pedido não existe na mensagem. Sprint #589.
#[derive(Debug, serde::Deserialize)]
struct BodyParams {
    format: Option<String>,
}

async fn message_body(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Query(params): Query<BodyParams>,
) -> Result<Json<serde_json::Value>> {
    let format = params.format.as_deref().unwrap_or("text");
    if format != "text" && format != "html" {
        return Err(MailError::InvalidMessage("format must be 'text' or 'html'".into()));
    }

    let (body_path, _) = fetch_message_meta(&state, &ctx, id).await?;
    let raw = load_raw(&state, &body_path).await?;
    let msg = MessageParser::default()
        .parse(&raw)
        .ok_or_else(|| MailError::InvalidMessage("failed to parse MIME".into()))?;

    let body = if format == "html" {
        msg.body_html(0).map(|s| s.into_owned())
    } else {
        msg.body_text(0).map(|s| s.into_owned())
    };

    let body = body.ok_or_else(|| {
        MailError::InvalidMessage(format!("no {format} body part found in message"))
    })?;

    Ok(Json(serde_json::json!({
        "message_id": id,
        "format":     format,
        "body":       body,
    })))
}

/// GET /api/v1/mail/messages/:id/headers — headers parsed de uma mensagem.
///
/// Retorna `{message_id, headers: [{name, value}]}` com todos os headers RFC 5322
/// da mensagem na ordem em que aparecem no raw `.eml`. Valores multi-ocorrência
/// (ex.: `Received`) são listados individualmente. 404 se mensagem não pertence
/// ao tenant/user. Sprint #587.
async fn message_headers(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let (body_path, _) = fetch_message_meta(&state, &ctx, id).await?;
    let raw = load_raw(&state, &body_path).await?;
    let msg = MessageParser::default()
        .parse(&raw)
        .ok_or_else(|| MailError::InvalidMessage("failed to parse MIME".into()))?;

    let headers: Vec<serde_json::Value> = msg
        .headers()
        .iter()
        .map(|h| {
            let name  = h.name();
            let value = h.value().as_text().unwrap_or("").to_string();
            serde_json::json!({"name": name, "value": value})
        })
        .collect();

    Ok(Json(serde_json::json!({"message_id": id, "headers": headers})))
}

/// GET /api/v1/mail/messages/:id/structure — MIME tree estrutural sem conteúdo.
///
/// Retorna `{message_id, parts: [node]}` onde cada nó é:
/// `{index, content_type, is_attachment, filename?, parts?: [node]}`.
/// `parts` está presente e não-vazio apenas em nós multipart.
/// Sprint #596.
async fn message_structure(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let (body_path, _) = fetch_message_meta(&state, &ctx, id).await?;
    let raw = load_raw(&state, &body_path).await?;
    let msg = MessageParser::default()
        .parse(&raw)
        .ok_or_else(|| MailError::InvalidMessage("failed to parse MIME".into()))?;

    fn build_node(msg: &mail_parser::Message, idx: usize) -> serde_json::Value {
        let part = match msg.part(idx) {
            Some(p) => p,
            None    => return serde_json::json!({"index": idx, "content_type": "unknown"}),
        };
        let ct = part.content_type().map(|c| format_ct(c))
            .unwrap_or_else(|| "application/octet-stream".into());
        let is_attachment = part.attachment_name().is_some();
        let filename = part.attachment_name().map(str::to_owned);

        match &part.body {
            PartType::Multipart(children) => {
                let child_nodes: Vec<serde_json::Value> = children.iter()
                    .map(|&ci| build_node(msg, ci))
                    .collect();
                serde_json::json!({
                    "index":         idx,
                    "content_type":  ct,
                    "is_attachment": is_attachment,
                    "filename":      filename,
                    "parts":         child_nodes,
                })
            }
            _ => serde_json::json!({
                "index":         idx,
                "content_type":  ct,
                "is_attachment": is_attachment,
                "filename":      filename,
            }),
        }
    }

    // Root is always part 0 in mail-parser (the message root).
    let root = build_node(&msg, 0);

    Ok(Json(serde_json::json!({"message_id": id, "structure": root})))
}

/// GET /api/v1/mail/messages/:id/inline-images
///
/// Lists MIME parts that have `Content-Disposition: inline` and a `Content-ID`
/// header (`cid:` reference used by HTML bodies to embed images inline). Returns
/// `{message_id, inline_images: [{index, content_type, content_id, filename?, size}]}`.
/// Parts without Content-ID are excluded — they are inline but not CID-referenced.
/// Useful for HTML rendering clients that need to resolve `<img src="cid:…">`.
/// 404 if message not found or not owned by the caller. Sprint #615.
async fn list_inline_images(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let (body_path, _) = fetch_message_meta(&state, &ctx, id).await?;
    let raw = load_raw(&state, &body_path).await?;
    let msg = MessageParser::default()
        .parse(&raw)
        .ok_or_else(|| MailError::InvalidMessage("failed to parse MIME".into()))?;

    let inline_images: Vec<serde_json::Value> = msg
        .parts
        .iter()
        .enumerate()
        .filter_map(|(i, part)| {
            // Must have Content-ID to be a CID-referenceable inline image.
            let cid = part.content_id()?;
            // Must be Content-Disposition: inline (or no explicit disposition but has CID).
            let is_inline = part
                .content_disposition()
                .map(|cd| cd.is_inline())
                .unwrap_or(true); // no explicit disposition + has CID → treat as inline
            if !is_inline {
                return None;
            }
            let ct       = part.content_type().map(format_ct)
                .unwrap_or_else(|| "application/octet-stream".into());
            let filename = part.attachment_name().map(str::to_owned);
            let size     = part.len();
            Some(serde_json::json!({
                "index":        i,
                "content_type": ct,
                "content_id":   cid,
                "filename":     filename,
                "size":         size,
            }))
        })
        .collect();

    Ok(Json(serde_json::json!({
        "message_id":    id,
        "inline_images": inline_images,
    })))
}

/// GET /api/v1/mail/messages/:id/inline-images/:index — download binary content of
/// a CID-referenceable inline image part by its index in `msg.parts`.
///
/// Complements `GET /inline-images` (list) with actual blob delivery.
/// Content-Disposition is `inline` so clients can render directly.
/// 404 if message not found or index has no Content-ID (not an inline image).
/// Sprint #620.
async fn download_inline_image(
    State(state):       State<AppState>,
    ctx:                RequestCtx,
    Path((id, index)):  Path<(Uuid, usize)>,
) -> Result<Response> {
    let (body_path, _) = fetch_message_meta(&state, &ctx, id).await?;
    let raw = load_raw(&state, &body_path).await?;
    let msg = MessageParser::default()
        .parse(&raw)
        .ok_or_else(|| MailError::InvalidMessage("failed to parse MIME".into()))?;

    let part = msg.parts.get(index)
        .ok_or_else(|| MailError::InvalidMessage(format!("index {index} out of range")))?;

    // Must be CID-referenceable.
    let _cid = part.content_id()
        .ok_or_else(|| MailError::InvalidMessage(format!("part at index {index} has no Content-ID")))?;

    let is_inline = part.content_disposition()
        .map(|cd| cd.is_inline())
        .unwrap_or(true);
    if !is_inline {
        return Err(MailError::InvalidMessage(format!("part at index {index} is not inline")));
    }

    let ct = part.content_type().map(format_ct)
        .unwrap_or_else(|| "application/octet-stream".into());
    let filename = part.attachment_name().unwrap_or("inline").to_owned();
    let body = part.contents().to_vec();

    let ct_safe = sanitize_header_token(&ct, "application/octet-stream");
    let ascii: String = filename.chars().map(|c| {
        if c.is_ascii_graphic() && c != '"' && c != '\\' { c } else { '_' }
    }).collect();
    let ascii = if ascii.trim().is_empty() { "inline".to_string() } else { ascii };
    let cd = format!("inline; filename=\"{ascii}\"");

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE,        ct_safe),
            (header::CONTENT_DISPOSITION, cd),
        ],
        body,
    ).into_response())
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

#[cfg(test)]
mod extra_tests {
    use super::*;

    #[test]
    fn sanitize_header_token_strips_control_chars() {
        let s = sanitize_header_token("application/pdf", "text/plain");
        assert_eq!(s, "application/pdf");
    }

    #[test]
    fn sanitize_header_token_empty_returns_default() {
        assert_eq!(sanitize_header_token("", "text/plain"), "text/plain");
        assert_eq!(sanitize_header_token("  ", "text/plain"), "text/plain");
    }

    #[test]
    fn build_content_disposition_ascii_filename() {
        let cd = build_content_disposition("report.pdf");
        assert!(cd.starts_with("attachment; filename=\"report.pdf\""));
        assert!(cd.contains("filename*=UTF-8''"));
    }

    #[test]
    fn build_content_disposition_empty_name_uses_fallback() {
        let cd = build_content_disposition("");
        assert!(cd.contains("filename=\"attachment\""));
    }

    #[test]
    fn percent_encode_filename_ascii_chars_unchanged() {
        let s = percent_encode_filename("file.txt");
        assert_eq!(s, "file.txt");
    }

    #[test]
    fn percent_encode_filename_non_ascii_encoded() {
        let s = percent_encode_filename("relatório.pdf");
        assert!(s.contains('%'));
    }

    #[test]
    fn percent_encode_filename_space_encoded() {
        let s = percent_encode_filename("my file.pdf");
        assert!(s.contains('%'));
        assert!(!s.contains(' '));
    }

    #[test]
    fn percent_encode_filename_alphanumeric_unchanged() {
        let s = percent_encode_filename("report2026.pdf");
        assert_eq!(s, "report2026.pdf");
    }

    #[test]
    fn percent_encode_filename_space_is_encoded() {
        let s = percent_encode_filename("my file.pdf");
        assert!(!s.contains(' '));
    }

    #[test]
    fn percent_encode_filename_empty_returns_empty() {
        assert_eq!(percent_encode_filename(""), "");
    }

    #[test]
    fn percent_encode_filename_dot_preserved() {
        let s = percent_encode_filename("archive.tar.gz");
        assert_eq!(s, "archive.tar.gz");
    }

    #[test]
    fn percent_encode_filename_hyphen_preserved() {
        let s = percent_encode_filename("my-file.pdf");
        assert_eq!(s, "my-file.pdf");
    }

    #[test]
    fn percent_encode_filename_underscore_preserved() {
        let s = percent_encode_filename("my_file.pdf");
        assert_eq!(s, "my_file.pdf");
    }

    #[test]
    fn percent_encode_filename_tilde_preserved() {
        let s = percent_encode_filename("~draft.txt");
        assert_eq!(s, "~draft.txt");
    }
}

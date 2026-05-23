//! Draft management endpoints.
//!
//! POST   /api/v1/mail/drafts       — save new draft to \Drafts folder
//! PUT    /api/v1/mail/drafts/:id   — replace existing draft (delete + insert)
//! DELETE /api/v1/mail/drafts/:id   — discard draft

use axum::{
    Router,
    routing::{delete, post, put},
    extract::{State, Path},
    Json, http::StatusCode,
};
use expresso_core::begin_tenant_tx;
use lettre::{
    message::{header::ContentType, Mailbox, Message, MultiPart, SinglePart},
    Address,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    api::context::RequestCtx,
    error::{MailError, Result},
    ingest,
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/drafts",      post(save_draft))
        .route("/mail/drafts/:id",  put(update_draft).delete(discard_draft))
}

#[derive(Debug, Deserialize)]
pub struct DraftRequest {
    pub from:       String,
    pub to:         Option<Vec<String>>,
    pub cc:         Option<Vec<String>>,
    pub bcc:        Option<Vec<String>>,
    pub subject:    Option<String>,
    pub body_text:  Option<String>,
    pub body_html:  Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DraftCreated {
    pub id: Uuid,
}

/// POST /api/v1/mail/drafts — serialize to RFC 2822, store in \Drafts with \Draft flag.
async fn save_draft(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(req):    Json<DraftRequest>,
) -> Result<(StatusCode, Json<DraftCreated>)> {
    let raw = build_raw(&req)?;
    let id = store_draft(&state, &ctx, &raw, None).await?;
    Ok((StatusCode::CREATED, Json(DraftCreated { id })))
}

/// PUT /api/v1/mail/drafts/:id — replace: delete old draft, store new one.
async fn update_draft(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(old_id): Path<Uuid>,
    Json(req):    Json<DraftRequest>,
) -> Result<Json<DraftCreated>> {
    let raw = build_raw(&req)?;
    // Delete old draft if it belongs to this user.
    {
        let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
        sqlx::query(
            "DELETE FROM messages \
             WHERE id = $1 AND tenant_id = $2 \
               AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $3 AND tenant_id = $2)",
        )
        .bind(old_id)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }
    let id = store_draft(&state, &ctx, &raw, None).await?;
    Ok(Json(DraftCreated { id }))
}

/// DELETE /api/v1/mail/drafts/:id — discard a draft message.
async fn discard_draft(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<StatusCode> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    sqlx::query(
        "DELETE FROM messages \
         WHERE id = $1 AND tenant_id = $2 \
           AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $3 AND tenant_id = $2)",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Build RFC 2822 bytes from a draft request using lettre.
fn build_raw(req: &DraftRequest) -> Result<Vec<u8>> {
    let from_addr: Address = req.from.parse()
        .map_err(|_| MailError::InvalidMessage(format!("invalid from: {}", req.from)))?;

    let mut builder = Message::builder()
        .from(Mailbox::new(None, from_addr))
        .subject(req.subject.as_deref().unwrap_or("(no subject)"));

    for addr_str in req.to.iter().flatten() {
        let a: Address = addr_str.parse()
            .map_err(|_| MailError::InvalidMessage(format!("invalid to: {addr_str}")))?;
        builder = builder.to(Mailbox::new(None, a));
    }
    for addr_str in req.cc.iter().flatten() {
        let a: Address = addr_str.parse()
            .map_err(|_| MailError::InvalidMessage(format!("invalid cc: {addr_str}")))?;
        builder = builder.cc(Mailbox::new(None, a));
    }
    for addr_str in req.bcc.iter().flatten() {
        let a: Address = addr_str.parse()
            .map_err(|_| MailError::InvalidMessage(format!("invalid bcc: {addr_str}")))?;
        builder = builder.bcc(Mailbox::new(None, a));
    }

    let email = match (req.body_html.as_deref(), req.body_text.as_deref()) {
        (Some(html), Some(plain)) => builder.multipart(
            MultiPart::alternative()
                .singlepart(SinglePart::builder().header(ContentType::TEXT_PLAIN).body(plain.to_string()))
                .singlepart(SinglePart::builder().header(ContentType::TEXT_HTML).body(html.to_string())),
        ),
        (Some(html), None) => builder.singlepart(
            SinglePart::builder().header(ContentType::TEXT_HTML).body(html.to_string()),
        ),
        (None, plain_opt) => builder.singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_PLAIN)
                .body(plain_opt.unwrap_or("").to_string()),
        ),
    }
    .map_err(|e| MailError::InvalidMessage(e.to_string()))?;

    Ok(email.formatted())
}

/// Store raw RFC 2822 bytes into the user's \Drafts mailbox with \Draft flag.
/// Returns the new message UUID.
async fn store_draft(
    state: &AppState,
    ctx:   &RequestCtx,
    raw:   &[u8],
    _id:   Option<Uuid>,
) -> Result<Uuid> {
    let body_path = ingest::write_raw_message(state, raw).await
        .map_err(|e| MailError::SendFailed(e.to_string()))?;

    let size_bytes = raw.len().min(i32::MAX as usize) as i32;
    let msg_id = Uuid::now_v7();

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    // Lock the \Drafts mailbox row to get a UID safely, creating it if absent.
    let row: Option<(Uuid, i64)> = sqlx::query_as(
        "SELECT id, next_uid FROM mailboxes \
         WHERE user_id = $1 AND tenant_id = $2 AND special_use = $3 FOR UPDATE",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(r"\Drafts")
    .fetch_optional(&mut *tx)
    .await?;

    let (mbox_id, uid) = if let Some((mid, nu)) = row {
        (mid, nu)
    } else {
        // Auto-create Drafts mailbox on first draft save.
        let mid: Uuid = sqlx::query_scalar(
            "INSERT INTO mailboxes \
               (user_id, tenant_id, folder_name, special_use, uid_validity, next_uid, subscribed) \
             VALUES ($1, $2, 'Drafts', $3, EXTRACT(EPOCH FROM now())::BIGINT, 1, true) \
             RETURNING id",
        )
        .bind(ctx.user_id)
        .bind(ctx.tenant_id)
        .bind(r"\Drafts")
        .fetch_one(&mut *tx)
        .await?;
        (mid, 1i64)
    };

    sqlx::query(
        "UPDATE mailboxes SET next_uid = next_uid + 1 WHERE id = $1",
    )
    .bind(mbox_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO messages (id, mailbox_id, tenant_id, uid, flags, size_bytes, body_path, received_at) \
         VALUES ($1, $2, $3, $4, ARRAY[$5::text], $6, $7, now())",
    )
    .bind(msg_id)
    .bind(mbox_id)
    .bind(ctx.tenant_id)
    .bind(uid)
    .bind(r"\Draft")
    .bind(size_bytes)
    .bind(&body_path)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(msg_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(from: &str, to: Option<Vec<&str>>, subject: Option<&str>, body_text: Option<&str>) -> DraftRequest {
        DraftRequest {
            from:      from.into(),
            to:        to.map(|v| v.iter().map(|s| s.to_string()).collect()),
            cc:        None,
            bcc:       None,
            subject:   subject.map(|s| s.into()),
            body_text: body_text.map(|s| s.into()),
            body_html: None,
        }
    }

    #[test]
    fn build_raw_minimal_produces_bytes() {
        let r = req("user@example.com", None, Some("Test"), Some("hello"));
        let bytes = build_raw(&r).expect("build_raw must succeed for valid addr");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn build_raw_invalid_from_returns_error() {
        let r = req("not-an-email", None, None, None);
        assert!(build_raw(&r).is_err());
    }

    #[test]
    fn build_raw_with_recipients_succeeds() {
        let r = req("a@example.com", Some(vec!["b@example.com", "c@example.com"]), None, None);
        assert!(build_raw(&r).is_ok());
    }

    #[test]
    fn build_raw_invalid_to_returns_error() {
        let mut r = req("a@example.com", None, None, None);
        r.to = Some(vec!["bad-address".into()]);
        assert!(build_raw(&r).is_err());
    }

    #[test]
    fn build_raw_no_subject_defaults() {
        // subject = None → "(no subject)" used internally — must not error
        let r = req("a@example.com", None, None, Some("body"));
        assert!(build_raw(&r).is_ok());
    }

    #[test]
    fn build_raw_output_contains_from_header() {
        let r = req("sender@example.com", None, Some("Subj"), Some("body"));
        let bytes = build_raw(&r).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("sender@example.com"));
    }

    #[test]
    fn build_raw_subject_appears_in_output() {
        let r = req("a@example.com", None, Some("My Subject"), None);
        let bytes = build_raw(&r).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("My Subject"));
    }

    #[test]
    fn build_raw_body_text_appears_in_output() {
        let r = req("a@example.com", None, None, Some("Hello from draft"));
        let bytes = build_raw(&r).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("Hello from draft"));
    }

    #[test]
    fn build_raw_multiple_recipients_all_appear() {
        let r = req("a@example.com", Some(vec!["b@x.com", "c@x.com"]), None, None);
        let bytes = build_raw(&r).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("b@x.com"));
        assert!(s.contains("c@x.com"));
    }

    #[test]
    fn build_raw_from_addr_appears_in_output() {
        let r = req("sender@test.com", None, None, None);
        let s = String::from_utf8_lossy(&build_raw(&r).unwrap());
        assert!(s.contains("sender@test.com"));
    }

    #[test]
    fn build_raw_produces_nonempty_output() {
        let r = req("any@ex.com", None, None, None);
        assert!(!build_raw(&r).unwrap().is_empty());
    }

    #[test]
    fn build_raw_subject_in_output() {
        let r = req("s@example.com", None, Some("Meeting notes"), None);
        let s = String::from_utf8_lossy(&build_raw(&r).unwrap());
        assert!(s.contains("Meeting notes"));
    }

    #[test]
    fn build_raw_empty_recipients_succeeds() {
        let r = req("from@example.com", Some(vec![]), None, None);
        assert!(build_raw(&r).is_ok());
    }

    #[test]
    fn build_raw_with_subject_succeeds() {
        let r = req("from@example.com", None, Some("Test subject"), None);
        assert!(build_raw(&r).is_ok());
    }

    #[test]
    fn build_raw_body_appears_in_output() {
        let r = req("from@example.com", None, None, Some("Draft body text"));
        let s = String::from_utf8_lossy(&build_raw(&r).unwrap());
        assert!(s.contains("Draft body text"));
    }

    #[test]
    fn build_raw_contains_mime_version_header() {
        let r = req("from@example.com", None, None, None);
        let s = String::from_utf8_lossy(&build_raw(&r).unwrap());
        assert!(s.to_ascii_lowercase().contains("mime-version"));
    }

    #[test]
    fn build_raw_invalid_cc_returns_error() {
        let mut r = req("from@example.com", None, None, None);
        r.cc = Some(vec!["not-an-address".into()]);
        assert!(build_raw(&r).is_err());
    }

    #[test]
    fn build_raw_invalid_bcc_returns_error() {
        let mut r = req("from@example.com", None, None, None);
        r.bcc = Some(vec!["not-valid".into()]);
        assert!(build_raw(&r).is_err());
    }

    #[test]
    fn draft_request_from_field_preserved() {
        let r = req("author@example.com", None, None, None);
        assert_eq!(r.from, "author@example.com");
    }

    #[test]
    fn build_raw_with_bcc_succeeds() {
        let mut r = req("from@example.com", None, None, None);
        r.bcc = Some(vec!["bcc@example.com".into()]);
        assert!(build_raw(&r).is_ok());
    }

    #[test]
    fn build_raw_with_cc_succeeds() {
        let mut r = req("from@example.com", None, None, None);
        r.cc = Some(vec!["cc@example.com".into()]);
        assert!(build_raw(&r).is_ok());
    }

    #[test]
    fn build_raw_with_bcc_and_subject_succeeds() {
        let mut r = req("from@example.com", None, Some("Hello"), None);
        r.bcc = Some(vec!["bcc@example.com".into()]);
        assert!(build_raw(&r).is_ok());
    }

    #[test]
    fn draft_request_cc_none_by_default() {
        let r = req("from@example.com", None, None, None);
        assert!(r.cc.is_none());
    }

    #[test]
    fn draft_request_bcc_none_by_default() {
        let r = req("from@example.com", None, None, None);
        assert!(r.bcc.is_none());
    }
}

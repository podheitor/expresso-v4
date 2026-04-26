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

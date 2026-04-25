//! Message list, read, delete, move endpoints.
//!
//! Tenant scoping: cada handler abre transação via `begin_tenant_tx` para
//! defense-in-depth e aplica `WHERE tenant_id = $1` explícito. Handlers que
//! navegam por mailboxes também checam `mailboxes.user_id = $2` para isolar
//! entre usuários do mesmo tenant — sem isso, qualquer usuário autenticado
//! listava/lia/alterava mensagens de qualquer outro (RLS de `messages` e
//! `mailboxes` é NULL-bypass).

use axum::{
    Router,
    routing::{get, delete, patch},
    extract::{State, Path, Query},
    Json, http::StatusCode,
};
use expresso_core::begin_tenant_tx;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{api::context::RequestCtx, error::{MailError, Result}, state::AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/messages",            get(list_messages))
        .route("/mail/threads/:thread_id",  get(list_thread))
        .route("/mail/messages/:id",        get(get_message))
        .route("/mail/messages/:id",        delete(delete_message))
        .route("/mail/messages/:id/move",   patch(move_message))
        .route("/mail/messages/:id/flags",  patch(update_flags))
}

// ─── DTOs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListParams {
    pub folder:  Option<String>,
    pub page:    Option<i64>,
    pub limit:   Option<i64>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct MessageListItem {
    pub id:              Uuid,
    pub thread_id:       Option<Uuid>,
    pub subject:         Option<String>,
    pub from_addr:       Option<String>,
    pub from_name:       Option<String>,
    pub has_attachments: bool,
    pub preview_text:    Option<String>,
    pub flags:           Vec<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub date:            Option<OffsetDateTime>,
    pub size_bytes:      i32,
}

#[derive(Debug, Serialize, FromRow)]
pub struct MessageDetail {
    pub id:              Uuid,
    pub mailbox_id:      Uuid,
    pub subject:         Option<String>,
    pub from_addr:       Option<String>,
    pub from_name:       Option<String>,
    pub to_addrs:        serde_json::Value,
    pub cc_addrs:        serde_json::Value,
    pub reply_to:        Option<String>,
    pub message_id:      Option<String>,
    pub thread_id:       Option<Uuid>,
    pub flags:           Vec<String>,
    pub has_attachments: bool,
    pub body_path:       String,
    pub preview_text:    Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub date:            Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub received_at:     OffsetDateTime,
    pub size_bytes:      i32,
}

#[derive(Debug, Deserialize)]
pub struct MoveRequest {
    pub target_folder: String,
}

#[derive(Debug, Deserialize)]
pub struct FlagRequest {
    pub add:    Vec<String>,
    pub remove: Vec<String>,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// GET /api/v1/mail/messages?folder=INBOX&page=0&limit=50
async fn list_messages(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<MessageListItem>>> {
    let folder = params.folder.unwrap_or_else(|| "INBOX".into());
    let limit  = params.limit.unwrap_or(50).min(200);
    let offset = params.page.unwrap_or(0) * limit;

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let rows: Vec<MessageListItem> = sqlx::query_as(
        r#"
        SELECT
            m.id, m.thread_id, m.subject, m.from_addr, m.from_name,
            m.has_attachments, m.preview_text, m.flags, m.date, m.size_bytes
        FROM messages  m
        JOIN mailboxes mb ON mb.id = m.mailbox_id
        WHERE m.tenant_id    = $1
          AND mb.tenant_id   = $1
          AND mb.user_id     = $2
          AND mb.folder_name = $3
        ORDER BY m.received_at DESC
        LIMIT $4 OFFSET $5
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&folder)
    .bind(limit)
    .bind(offset)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(rows))
}

/// GET /api/v1/mail/messages/:id — mark as Seen + return detail
async fn get_message(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<MessageDetail>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let msg: Option<MessageDetail> = sqlx::query_as(
        r#"
        SELECT m.id, m.mailbox_id, m.subject, m.from_addr, m.from_name,
               m.to_addrs, m.cc_addrs, m.reply_to, m.message_id, m.thread_id,
               m.flags, m.has_attachments, m.body_path, m.preview_text,
               m.date, m.received_at, m.size_bytes
        FROM messages  m
        JOIN mailboxes mb ON mb.id = m.mailbox_id
        WHERE m.id         = $1
          AND m.tenant_id  = $2
          AND mb.tenant_id = $2
          AND mb.user_id   = $3
        "#,
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let msg = msg.ok_or(MailError::MessageNotFound(id))?;

    if !msg.flags.iter().any(|f| f == r"\Seen") {
        let _ = sqlx::query(
            r#"UPDATE messages
               SET flags = array_append(flags, $1)
               WHERE id = $2 AND tenant_id = $3 AND NOT ($1 = ANY(flags))"#,
        )
        .bind(r"\Seen")
        .bind(id)
        .bind(ctx.tenant_id)
        .execute(&mut *tx)
        .await;
    }
    tx.commit().await?;

    Ok(Json(msg))
}


/// GET /api/v1/mail/threads/:thread_id — list all messages in thread ordered ASC
async fn list_thread(
    State(state):    State<AppState>,
    ctx:             RequestCtx,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<Vec<MessageListItem>>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let rows: Vec<MessageListItem> = sqlx::query_as(
        r#"
        SELECT
            m.id, m.thread_id, m.subject, m.from_addr, m.from_name,
            m.has_attachments, m.preview_text, m.flags, m.date, m.size_bytes
        FROM messages  m
        JOIN mailboxes mb ON mb.id = m.mailbox_id
        WHERE m.thread_id  = $1
          AND m.tenant_id  = $2
          AND mb.tenant_id = $2
          AND mb.user_id   = $3
        ORDER BY m.received_at ASC
        "#,
    )
    .bind(thread_id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(rows))
}

/// DELETE /api/v1/mail/messages/:id — soft-delete: move to Trash
async fn delete_message(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<StatusCode> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let trash_id: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT id FROM mailboxes
            WHERE tenant_id   = $1
              AND user_id     = $2
              AND special_use = $3
            LIMIT 1"#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(r"\Trash")
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(trash) = trash_id {
        sqlx::query(
            r#"UPDATE messages SET mailbox_id = $1
                WHERE id = $2 AND tenant_id = $3"#,
        )
        .bind(trash)
        .bind(id)
        .bind(ctx.tenant_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

/// PATCH /api/v1/mail/messages/:id/move
async fn move_message(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<MoveRequest>,
) -> Result<StatusCode> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let target_id: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT id FROM mailboxes
            WHERE tenant_id   = $1
              AND user_id     = $2
              AND folder_name = $3
            LIMIT 1"#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&body.target_folder)
    .fetch_optional(&mut *tx)
    .await?;

    let target_id = target_id.ok_or_else(|| MailError::FolderNotFound {
        folder: body.target_folder.clone(),
    })?;

    sqlx::query(
        r#"UPDATE messages SET mailbox_id = $1
            WHERE id = $2 AND tenant_id = $3"#,
    )
    .bind(target_id)
    .bind(id)
    .bind(ctx.tenant_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

/// PATCH /api/v1/mail/messages/:id/flags
async fn update_flags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<FlagRequest>,
) -> Result<StatusCode> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    if !body.add.is_empty() {
        sqlx::query(
            r#"UPDATE messages
                SET flags = array(SELECT DISTINCT unnest(flags || $1::text[]))
                WHERE id = $2 AND tenant_id = $3"#,
        )
        .bind(&body.add)
        .bind(id)
        .bind(ctx.tenant_id)
        .execute(&mut *tx)
        .await?;
    }
    if !body.remove.is_empty() {
        sqlx::query(
            r#"UPDATE messages
                SET flags = array(SELECT unnest(flags) EXCEPT SELECT unnest($1::text[]))
                WHERE id = $2 AND tenant_id = $3"#,
        )
        .bind(&body.remove)
        .bind(id)
        .bind(ctx.tenant_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

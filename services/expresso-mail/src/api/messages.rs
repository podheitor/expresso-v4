//! Message list, read, delete, move endpoints

use axum::{
    Router,
    routing::{get, delete, patch},
    extract::{State, Path, Query},
    Json, http::StatusCode,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{error::{MailError, Result}, state::AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/messages",            get(list_messages))
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
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<MessageListItem>>> {
    let folder = params.folder.unwrap_or_else(|| "INBOX".into());
    let limit  = params.limit.unwrap_or(50).min(200);
    let offset = params.page.unwrap_or(0) * limit;

    let rows: Vec<MessageListItem> = sqlx::query_as(
        r#"
        SELECT
            m.id, m.subject, m.from_addr, m.from_name,
            m.has_attachments, m.preview_text, m.flags, m.date, m.size_bytes
        FROM messages m
        JOIN mailboxes mb ON mb.id = m.mailbox_id
        WHERE mb.folder_name = $1
        ORDER BY m.received_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(&folder)
    .bind(limit)
    .bind(offset)
    .fetch_all(state.db())
    .await
    .map_err(expresso_core::CoreError::Database)?;

    Ok(Json(rows))
}

/// GET /api/v1/mail/messages/:id — mark as Seen + return detail
async fn get_message(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageDetail>> {
    let msg: Option<MessageDetail> = sqlx::query_as(
        r#"
        SELECT id, mailbox_id, subject, from_addr, from_name,
               to_addrs, cc_addrs, reply_to, message_id, thread_id,
               flags, has_attachments, body_path, preview_text,
               date, received_at, size_bytes
        FROM messages WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(state.db())
    .await
    .map_err(expresso_core::CoreError::Database)?;

    let msg = msg.ok_or(MailError::MessageNotFound(id))?;

    // Auto-mark Seen if not already set
    if !msg.flags.iter().any(|f| f == r"\Seen") {
        let _ = sqlx::query(
            r#"UPDATE messages
               SET flags = array_append(flags, $1)
               WHERE id = $2 AND NOT ($1 = ANY(flags))"#,
        )
        .bind(r"\Seen")
        .bind(id)
        .execute(state.db())
        .await;
    }

    Ok(Json(msg))
}

/// DELETE /api/v1/mail/messages/:id — soft-delete: move to Trash
async fn delete_message(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    // Find trash mailbox (scoped by RLS to current tenant)
    let trash_id: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT id FROM mailboxes WHERE special_use = $1 LIMIT 1"#,
    )
    .bind(r"\Trash")
    .fetch_optional(state.db())
    .await
    .map_err(expresso_core::CoreError::Database)?;

    if let Some(trash) = trash_id {
        sqlx::query("UPDATE messages SET mailbox_id = $1 WHERE id = $2")
            .bind(trash)
            .bind(id)
            .execute(state.db())
            .await
            .map_err(expresso_core::CoreError::Database)?;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// PATCH /api/v1/mail/messages/:id/move
async fn move_message(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<MoveRequest>,
) -> Result<StatusCode> {
    let target_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM mailboxes WHERE folder_name = $1 LIMIT 1",
    )
    .bind(&body.target_folder)
    .fetch_optional(state.db())
    .await
    .map_err(expresso_core::CoreError::Database)?;

    let target_id = target_id.ok_or_else(|| MailError::FolderNotFound {
        folder: body.target_folder.clone(),
    })?;

    sqlx::query("UPDATE messages SET mailbox_id = $1 WHERE id = $2")
        .bind(target_id)
        .bind(id)
        .execute(state.db())
        .await
        .map_err(expresso_core::CoreError::Database)?;

    Ok(StatusCode::NO_CONTENT)
}

/// PATCH /api/v1/mail/messages/:id/flags
async fn update_flags(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<FlagRequest>,
) -> Result<StatusCode> {
    if !body.add.is_empty() {
        sqlx::query(
            "UPDATE messages SET flags = array(SELECT DISTINCT unnest(flags || $1::text[])) WHERE id = $2",
        )
        .bind(&body.add)
        .bind(id)
        .execute(state.db())
        .await
        .map_err(expresso_core::CoreError::Database)?;
    }
    if !body.remove.is_empty() {
        sqlx::query(
            "UPDATE messages SET flags = array(SELECT unnest(flags) EXCEPT SELECT unnest($1::text[])) WHERE id = $2",
        )
        .bind(&body.remove)
        .bind(id)
        .execute(state.db())
        .await
        .map_err(expresso_core::CoreError::Database)?;
    }

    Ok(StatusCode::NO_CONTENT)
}

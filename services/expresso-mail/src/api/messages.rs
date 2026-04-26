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
    routing::{get, delete, patch, post},
    extract::{State, Path, Query},
    response::{IntoResponse, Response},
    Json, http::{StatusCode, header},
    body::Body,
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
        .route("/mail/search",              get(search_messages))
        .route("/mail/threads/:thread_id",  get(list_thread))
        .route("/mail/messages/:id",        get(get_message))
        .route("/mail/messages/:id",        delete(delete_message))
        .route("/mail/messages/:id/raw",    get(get_message_raw))
        .route("/mail/messages/:id/move",   patch(move_message))
        .route("/mail/messages/:id/flags",  patch(update_flags))
        .route("/mail/messages/bulk",       post(bulk_action))
}

// ─── DTOs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListParams {
    pub folder:  Option<String>,
    pub page:    Option<i64>,
    pub limit:   Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    /// Full-text search in subject, from, and preview_text
    pub q:       Option<String>,
    pub folder:  Option<String>,
    pub from:    Option<String>,
    pub subject: Option<String>,
    /// ISO-8601 date string — messages received on or after
    pub since:   Option<String>,
    /// ISO-8601 date string — messages received before
    pub before:  Option<String>,
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
    pub in_reply_to:     Option<String>,
    pub references_:     Vec<String>,
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

/// GET /api/v1/mail/search?q=text&folder=INBOX&from=addr&subject=text&since=date&before=date
///
/// Full-text and envelope search across the user's mailbox.
/// `q` searches subject + from_addr + preview_text (ILIKE).
/// `since`/`before` are ISO-8601 date prefixes (YYYY-MM-DD).
async fn search_messages(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<MessageListItem>>> {
    let limit  = params.limit.unwrap_or(50).min(200);
    let offset = params.page.unwrap_or(0) * limit;

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let folder_filter = params.folder
        .map(|f| format!("AND mb.folder_name = '{}'", f.replace('\'', "''")))
        .unwrap_or_default();
    let q_filter = params.q.map(|q| {
        let esc = q.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
        format!("AND (m.subject ILIKE '%{esc}%' OR m.from_addr ILIKE '%{esc}%' OR m.preview_text ILIKE '%{esc}%')")
    }).unwrap_or_default();
    let from_filter = params.from.map(|f| {
        let esc = f.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
        format!("AND m.from_addr ILIKE '%{esc}%'")
    }).unwrap_or_default();
    let subject_filter = params.subject.map(|s| {
        let esc = s.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
        format!("AND m.subject ILIKE '%{esc}%'")
    }).unwrap_or_default();
    let since_filter = params.since
        .map(|d| format!("AND m.received_at >= '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let before_filter = params.before
        .map(|d| format!("AND m.received_at < '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();

    let sql = format!(
        "SELECT m.id, m.thread_id, m.subject, m.from_addr, m.from_name, \
                m.has_attachments, m.preview_text, m.flags, m.date, m.size_bytes \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
           {folder_filter} {q_filter} {from_filter} {subject_filter} \
           {since_filter} {before_filter} \
         ORDER BY m.received_at DESC \
         LIMIT {limit} OFFSET {offset}"
    );

    let rows: Vec<MessageListItem> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(rows))
}

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
               m.to_addrs, m.cc_addrs, m.reply_to, m.message_id, m.in_reply_to,
               COALESCE(m.references_, '{}') AS references_,
               m.thread_id,
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


/// GET /api/v1/mail/messages/:id/raw — download RFC 2822 bytes
///
/// Returns `Content-Type: message/rfc822` and
/// `Content-Disposition: attachment; filename="message.eml"`.
/// Fetches raw bytes from S3 or local filesystem via `body_path`.
/// Returns 404 if the message is not found or 502 if the body store is unavailable.
async fn get_message_raw(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Response> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        r#"SELECT m.body_path, m.message_id
           FROM messages  m
           JOIN mailboxes mb ON mb.id = m.mailbox_id
           WHERE m.id        = $1
             AND m.tenant_id = $2
             AND mb.user_id  = $3
           LIMIT 1"#,
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    let (body_path, message_id) = row.ok_or(MailError::MessageNotFound(id))?;

    let bytes = fetch_body_bytes_api(&state, &body_path).await
        .ok_or_else(|| MailError::SendFailed("body store unavailable".into()))?;

    let filename = message_id
        .as_deref()
        .map(|mid| {
            let clean: String = mid.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
                .collect();
            format!("{clean}.eml")
        })
        .unwrap_or_else(|| format!("{id}.eml"));

    let cd = format!("attachment; filename=\"{filename}\"");

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE,        "message/rfc822".to_string()),
            (header::CONTENT_DISPOSITION, cd),
        ],
        Body::from(bytes),
    ).into_response())
}

async fn fetch_body_bytes_api(state: &AppState, body_path: &str) -> Option<Vec<u8>> {
    if let Some(idx) = body_path.strip_prefix("s3://").and_then(|s| s.find('/').map(|i| "s3://".len() + i + 1)) {
        let key = &body_path[idx..];
        state.store()?.get(key).await.ok()
    } else if body_path.starts_with('/') {
        tokio::fs::read(body_path).await.ok()
    } else {
        None
    }
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
            "UPDATE messages SET mailbox_id = $1 \
             WHERE id = $2 AND tenant_id = $3 \
               AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $4 AND tenant_id = $3)",
        )
        .bind(trash)
        .bind(id)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
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
        "UPDATE messages SET mailbox_id = $1 \
         WHERE id = $2 AND tenant_id = $3 \
           AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $4 AND tenant_id = $3)",
    )
    .bind(target_id)
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
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
            "UPDATE messages \
             SET flags = array(SELECT DISTINCT unnest(flags || $1::text[])) \
             WHERE id = $2 AND tenant_id = $3 \
               AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $4 AND tenant_id = $3)",
        )
        .bind(&body.add)
        .bind(id)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .execute(&mut *tx)
        .await?;
    }
    if !body.remove.is_empty() {
        sqlx::query(
            "UPDATE messages \
             SET flags = array(SELECT unnest(flags) EXCEPT SELECT unnest($1::text[])) \
             WHERE id = $2 AND tenant_id = $3 \
               AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $4 AND tenant_id = $3)",
        )
        .bind(&body.remove)
        .bind(id)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Bulk ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum BulkRequest {
    Delete {
        ids: Vec<Uuid>,
    },
    Flag {
        ids:    Vec<Uuid>,
        add:    Vec<String>,
        remove: Vec<String>,
    },
    Move {
        ids:    Vec<Uuid>,
        folder: String,
    },
}

#[derive(Debug, Serialize)]
struct BulkResult {
    affected: u64,
}

/// POST /api/v1/mail/messages/bulk
///
/// Apply one action to a set of messages atomically.
/// `{"action":"delete","ids":[…]}` — soft-delete (sets expunged_at)
/// `{"action":"flag","ids":[…],"add":["\\Seen"],"remove":[]}` — update flags
/// `{"action":"move","ids":[…],"folder":"Trash"}` — move to folder
/// Returns `{"affected": N}` — count of rows modified.
async fn bulk_action(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<BulkRequest>,
) -> Result<Json<BulkResult>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    // Sub-select to verify ownership via mailboxes.user_id (messages has no user_id column).
    let owned_mboxes = "(SELECT id FROM mailboxes WHERE user_id = $U AND tenant_id = $T)";

    let affected = match &body {
        BulkRequest::Delete { ids } => {
            // Hard-delete owned messages matching the id list.
            let res = sqlx::query(
                "DELETE FROM messages \
                 WHERE id = ANY($1) AND tenant_id = $2 \
                   AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $3 AND tenant_id = $2)",
            )
            .bind(ids)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .execute(&mut *tx)
            .await?;
            let _ = owned_mboxes; // suppress unused warning
            res.rows_affected()
        }
        BulkRequest::Flag { ids, add, remove } => {
            let mut rows = 0u64;
            if !add.is_empty() {
                let res = sqlx::query(
                    "UPDATE messages \
                     SET flags = array(SELECT DISTINCT unnest(flags || $1::text[])) \
                     WHERE id = ANY($2) AND tenant_id = $3 \
                       AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $4 AND tenant_id = $3)",
                )
                .bind(add)
                .bind(ids)
                .bind(ctx.tenant_id)
                .bind(ctx.user_id)
                .execute(&mut *tx)
                .await?;
                rows += res.rows_affected();
            }
            if !remove.is_empty() {
                let res = sqlx::query(
                    "UPDATE messages \
                     SET flags = array(SELECT unnest(flags) EXCEPT SELECT unnest($1::text[])) \
                     WHERE id = ANY($2) AND tenant_id = $3 \
                       AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $4 AND tenant_id = $3)",
                )
                .bind(remove)
                .bind(ids)
                .bind(ctx.tenant_id)
                .bind(ctx.user_id)
                .execute(&mut *tx)
                .await?;
                rows += res.rows_affected();
            }
            rows
        }
        BulkRequest::Move { ids, folder } => {
            let mbox: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM mailboxes \
                 WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3",
            )
            .bind(ctx.user_id)
            .bind(ctx.tenant_id)
            .bind(folder)
            .fetch_optional(&mut *tx)
            .await?;
            let dst_id = mbox.ok_or_else(|| MailError::NotFound("folder not found".into()))?;
            let res = sqlx::query(
                "UPDATE messages SET mailbox_id = $1 \
                 WHERE id = ANY($2) AND tenant_id = $3 \
                   AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $4 AND tenant_id = $3)",
            )
            .bind(dst_id)
            .bind(ids)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .execute(&mut *tx)
            .await?;
            res.rows_affected()
        }
    };

    tx.commit().await?;
    Ok(Json(BulkResult { affected }))
}

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
    routing::{get, delete, head, patch, post},
    extract::{State, Path, Query},
    response::{IntoResponse, Response},
    Json, http::{StatusCode, header, HeaderMap, HeaderValue},
    body::Body,
};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor, address::Envelope, Address};
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
        .route("/mail/threads",             get(list_threads))
        .route("/mail/threads/:thread_id",  get(list_thread))
        .route("/mail/messages/:id",        get(get_message))
        .route("/mail/messages/:id",        delete(delete_message))
        .route("/mail/messages/:id/thread", get(get_message_thread))
        .route("/mail/messages/:id/raw",    get(get_message_raw).head(head_message_raw))
        .route("/mail/messages/:id/move",   patch(move_message))
        .route("/mail/messages/:id/flags",  get(get_message_flags).patch(update_flags))
        .route("/mail/messages/bulk",        post(bulk_action).delete(bulk_delete))
        .route("/mail/messages/bulk/flags", patch(bulk_update_flags))
        .route("/mail/messages/:id/read-receipt", post(send_read_receipt))
        .route("/mail/messages/stats",            get(message_stats))
        .route("/mail/messages/stats/flags",      get(flag_stats))
        .route("/mail/messages/stats/threads",    get(thread_stats))
        .route("/mail/messages/stats/senders",    get(sender_stats))
        .route("/mail/messages/stats/size",       get(size_stats))
        .route("/mail/messages/stats/attachments",    get(attachment_stats))
        .route("/mail/messages/stats/received-by-day",  get(received_by_day_stats))
        .route("/mail/messages/stats/threads-by-day",   get(threads_by_day_stats))
}

// ─── DTOs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListParams {
    pub folder:    Option<String>,
    /// Legacy offset-based page (0-indexed). Ignored when before_id or after_id is set.
    pub page:      Option<i64>,
    pub limit:     Option<i64>,
    /// Keyset cursor — return messages received strictly before this message (DESC order).
    pub before_id: Option<Uuid>,
    /// Keyset cursor — return messages received strictly after this message (ASC, then reversed).
    pub after_id:  Option<Uuid>,
    /// Filter by flag presence (e.g. `\Seen`, `\Starred`, `\Flagged`). URL-encode backslash.
    pub flag:      Option<String>,
    /// Multi-flag AND filter: comma-separated list of flags — all must be present.
    /// Example: `flags=%5CSeen,%5CFlagged` (URL-encoded backslashes).
    pub flags:     Option<String>,
    /// If `true`, return only messages NOT having `\Seen` flag.
    pub unread:    Option<bool>,
    /// Return only messages belonging to this thread.
    pub thread_id: Option<Uuid>,
    /// Sort order for offset pagination: "asc" or "desc" (default "desc").
    /// Ignored when keyset cursors (before_id/after_id) are used.
    pub sort:      Option<String>,
    /// ILIKE filter on from_addr field.
    pub from_addr:       Option<String>,
    /// ILIKE filter on subject field.
    pub subject:         Option<String>,
    /// ILIKE filter on cc_addrs jsonb array.
    pub cc_addr:         Option<String>,
    /// If set, return only messages with (true) or without (false) attachments.
    pub has_attachments: Option<bool>,
    /// Return only messages with size_bytes >= this value.
    pub size_min:        Option<i32>,
    /// Return only messages with size_bytes <= this value.
    pub size_max:        Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    /// Full-text search in subject, from, and preview_text
    pub q:         Option<String>,
    pub folder:    Option<String>,
    pub from:      Option<String>,
    pub subject:   Option<String>,
    /// ILIKE filter on cc_addrs jsonb array.
    pub cc_addr:   Option<String>,
    /// ISO-8601 date string — messages received on or after
    pub since:     Option<String>,
    /// ISO-8601 date string — messages received before
    pub before:    Option<String>,
    /// Legacy offset-based page (0-indexed). Ignored when before_id or after_id is set.
    pub page:      Option<i64>,
    pub limit:     Option<i64>,
    /// Keyset cursor — return messages received strictly before this message (DESC order).
    pub before_id: Option<Uuid>,
    /// Keyset cursor — return messages received strictly after this message (ASC, then reversed).
    pub after_id:  Option<Uuid>,
    /// Return only messages belonging to this thread.
    pub thread_id: Option<Uuid>,
    /// Sort order for offset pagination: "asc" or "desc" (default "desc").
    /// Ignored when keyset cursors (before_id/after_id) are used.
    pub sort:            Option<String>,
    /// If set, return only messages with (true) or without (false) attachments.
    pub has_attachments: Option<bool>,
    /// Return only messages with size_bytes >= this value.
    pub size_min:        Option<i32>,
    /// Return only messages with size_bytes <= this value.
    pub size_max:        Option<i32>,
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
    pub bcc_addrs:       serde_json::Value,
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
/// `size_min`/`size_max` filter by `size_bytes` (inclusive range, bytes).
/// Supports the same `before_id`/`after_id` keyset cursor as `/mail/messages`.
/// `sort=asc/desc` controls offset-mode order (default `desc`); ignored with keyset cursors.
async fn search_messages(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Query(params): Query<SearchParams>,
    req_headers:   HeaderMap,
) -> Result<Response> {
    let limit = params.limit.unwrap_or(50).min(200);

    // When a full-text query is provided and the search service is configured,
    // call Tantivy to get matching document_ids (mailbox_id/uid pairs) and
    // inject them as an SQL filter. Falls back to ILIKE if the service is
    // unavailable or the query is empty.
    let tantivy_filter: Option<String> = match &params.q {
        Some(q) if !q.trim().is_empty() => {
            let search_url   = state.cfg().search_url.clone();
            let search_token = state.cfg().search_token.clone();
            if !search_url.is_empty() {
                let client = reqwest::Client::new();
                let mut req = client.get(format!("{search_url}/api/v1/search"))
                    .query(&[
                        ("q",         q.as_str()),
                        ("tenant_id", &ctx.tenant_id.to_string()),
                        ("limit",     "200"),
                    ]);
                if !search_token.is_empty() { req = req.bearer_auth(&search_token); }
                match req.send().await {
                    Ok(resp) if resp.status().is_success() => {
                        #[derive(serde::Deserialize)]
                        struct SResp { hits: Vec<SHit> }
                        #[derive(serde::Deserialize)]
                        struct SHit { document_id: String }
                        match resp.json::<SResp>().await {
                            Ok(sr) if !sr.hits.is_empty() => {
                                // document_id = "mailbox_id/uid" — build
                                // AND (m.mailbox_id::text || '/' || m.uid::text) = ANY($ids)
                                let ids: Vec<String> = sr.hits.into_iter()
                                    .map(|h| h.document_id)
                                    .collect();
                                let literal = ids.iter()
                                    .map(|s| format!("'{}'", s.replace('\'', "''")))
                                    .collect::<Vec<_>>()
                                    .join(",");
                                Some(format!(
                                    "AND (m.mailbox_id::text || '/' || m.uid::text) IN ({literal})"
                                ))
                            }
                            Ok(_) => Some("AND FALSE".into()), // no hits → empty result
                            Err(_) => None,                    // parse error → fallback
                        }
                    }
                    _ => None, // service down → fallback to ILIKE
                }
            } else {
                None // search_url not configured → fallback to ILIKE
            }
        }
        _ => None,
    };

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let folder_filter = params.folder
        .map(|f| format!("AND mb.folder_name = '{}'", f.replace('\'', "''")))
        .unwrap_or_default();
    let q_filter = if let Some(ref tf) = tantivy_filter {
        tf.clone()
    } else {
        params.q.as_ref().map(|q| {
            let esc = q.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
            format!("AND (m.subject ILIKE '%{esc}%' OR m.from_addr ILIKE '%{esc}%' OR m.preview_text ILIKE '%{esc}%')")
        }).unwrap_or_default()
    };
    let from_filter = params.from.map(|f| {
        let esc = f.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
        format!("AND m.from_addr ILIKE '%{esc}%'")
    }).unwrap_or_default();
    let subject_filter = params.subject.map(|s| {
        let esc = s.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
        format!("AND m.subject ILIKE '%{esc}%'")
    }).unwrap_or_default();
    let cc_addr_filter = params.cc_addr.map(|c| {
        let esc = c.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
        format!("AND EXISTS (SELECT 1 FROM jsonb_array_elements_text(m.cc_addrs) t WHERE t ILIKE '%{esc}%')")
    }).unwrap_or_default();
    let since_filter = params.since
        .map(|d| format!("AND m.received_at >= '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let before_date_filter = params.before
        .map(|d| format!("AND m.received_at < '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let thread_id_filter = params.thread_id
        .map(|t| format!("AND m.thread_id = '{t}'"))
        .unwrap_or_default();

    let has_attachments_filter = match params.has_attachments {
        Some(true)  => "AND m.has_attachments = TRUE",
        Some(false) => "AND m.has_attachments = FALSE",
        None        => "",
    };
    let size_min_filter = params.size_min
        .map(|v| format!("AND m.size_bytes >= {v}"))
        .unwrap_or_default();
    let size_max_filter = params.size_max
        .map(|v| format!("AND m.size_bytes <= {v}"))
        .unwrap_or_default();

    let base_select =
        "SELECT m.id, m.thread_id, m.subject, m.from_addr, m.from_name, \
                m.has_attachments, m.preview_text, m.flags, m.date, m.size_bytes \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2";
    let enum_filters = format!(
        "{folder_filter} {q_filter} {from_filter} {subject_filter} {cc_addr_filter} {since_filter} {before_date_filter} {thread_id_filter} {has_attachments_filter} {size_min_filter} {size_max_filter}"
    );

    let max_sql = format!(
        "SELECT MAX(m.received_at) \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
         {enum_filters}"
    );
    let max_received: Option<OffsetDateTime> = sqlx::query_scalar(&max_sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap_or(None);

    if let Some(ts) = max_received {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        tx.commit().await?;
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

    let rows: Vec<MessageListItem> = if let Some(cursor_id) = params.before_id.or(params.after_id) {
        let is_before = params.before_id.is_some();

        let anchor: Option<(time::OffsetDateTime, Uuid)> = sqlx::query_as(
            "SELECT m.received_at, m.id \
             FROM messages m \
             JOIN mailboxes mb ON mb.id = m.mailbox_id \
             WHERE m.id = $1 AND m.tenant_id = $2 AND mb.user_id = $3 \
             LIMIT 1",
        )
        .bind(cursor_id)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_optional(&mut *tx)
        .await?;

        let (anchor_ts, anchor_id) = anchor.ok_or(MailError::MessageNotFound(cursor_id))?;

        if is_before {
            let sql = format!(
                "{base_select} {enum_filters} \
                 AND (m.received_at, m.id) < ($3::timestamptz, $4::uuid) \
                 ORDER BY m.received_at DESC, m.id DESC LIMIT {limit}"
            );
            sqlx::query_as(&sql)
                .bind(ctx.tenant_id)
                .bind(ctx.user_id)
                .bind(anchor_ts)
                .bind(anchor_id)
                .fetch_all(&mut *tx)
                .await?
        } else {
            let sql = format!(
                "{base_select} {enum_filters} \
                 AND (m.received_at, m.id) > ($3::timestamptz, $4::uuid) \
                 ORDER BY m.received_at ASC, m.id ASC LIMIT {limit}"
            );
            let mut rows: Vec<MessageListItem> = sqlx::query_as(&sql)
                .bind(ctx.tenant_id)
                .bind(ctx.user_id)
                .bind(anchor_ts)
                .bind(anchor_id)
                .fetch_all(&mut *tx)
                .await?;
            rows.reverse();
            rows
        }
    } else {
        let offset = params.page.unwrap_or(0) * limit;
        let order = if params.sort.as_deref().map(|s| s.eq_ignore_ascii_case("asc")).unwrap_or(false) {
            "ASC"
        } else {
            "DESC"
        };
        let sql = format!(
            "{base_select} {enum_filters} \
             ORDER BY m.received_at {order}, m.id {order} LIMIT {limit} OFFSET {offset}"
        );
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .fetch_all(&mut *tx)
            .await?
    };

    let count_sql = format!(
        "SELECT COUNT(*) \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
         {enum_filters}"
    );
    let total: i64 = sqlx::query_scalar(&count_sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap_or(0);

    tx.commit().await?;
    let mut resp = (
        StatusCode::OK,
        [(header::HeaderName::from_static("x-total-count"), total.to_string())],
        Json(rows),
    ).into_response();
    if let Some(ts) = max_received {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

/// GET /api/v1/mail/messages?folder=INBOX&limit=50[&before_id=UUID|&after_id=UUID|&page=0]
///
/// Supports two pagination modes:
///   - Keyset (preferred): pass `before_id` or `after_id` for O(log N) seeks.
///   - Offset (legacy): pass `page` (0-indexed). Slow on large mailboxes.
///
/// Optional filters: `flag=\Starred`, `unread=true`, `thread_id=UUID`, `from_addr=`, `subject=` (ILIKE).
/// Optional sort for offset mode: `sort=asc` (default `desc`). Keyset direction is cursor-driven.
async fn list_messages(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Query(params): Query<ListParams>,
    req_headers:   HeaderMap,
) -> Result<Response> {
    let folder = params.folder.unwrap_or_else(|| "INBOX".into());
    let limit  = params.limit.unwrap_or(50).min(200);

    // Build optional flag filters (no user-provided SQL, only escaped literals).
    let flag_filter = params.flag
        .map(|f| format!("AND '{}' = ANY(m.flags)", f.replace('\'', "''")))
        .unwrap_or_default();
    // Multi-flag AND: every flag in the comma-separated list must be present.
    let multi_flag_filter = params.flags
        .map(|raw| {
            raw.split(',')
                .map(|f| format!("AND '{}' = ANY(m.flags)", f.trim().replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let unread_filter = if params.unread.unwrap_or(false) {
        "AND NOT ('\\Seen' = ANY(m.flags))".to_string()
    } else {
        String::new()
    };
    let thread_id_filter = params.thread_id
        .map(|t| format!("AND m.thread_id = '{t}'"))
        .unwrap_or_default();
    let from_addr_filter = params.from_addr.map(|f| {
        let esc = f.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
        format!("AND m.from_addr ILIKE '%{esc}%'")
    }).unwrap_or_default();
    let subject_filter = params.subject.map(|s| {
        let esc = s.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
        format!("AND m.subject ILIKE '%{esc}%'")
    }).unwrap_or_default();
    let cc_addr_filter = params.cc_addr.map(|c| {
        let esc = c.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
        format!("AND EXISTS (SELECT 1 FROM jsonb_array_elements_text(m.cc_addrs) t WHERE t ILIKE '%{esc}%')")
    }).unwrap_or_default();
    let has_attachments_filter = match params.has_attachments {
        Some(true)  => "AND m.has_attachments = TRUE",
        Some(false) => "AND m.has_attachments = FALSE",
        None        => "",
    };
    let size_min_filter = params.size_min
        .map(|v| format!("AND m.size_bytes >= {v}"))
        .unwrap_or_default();
    let size_max_filter = params.size_max
        .map(|v| format!("AND m.size_bytes <= {v}"))
        .unwrap_or_default();

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let max_received: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(m.received_at) FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 AND mb.folder_name = $3",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&folder)
    .fetch_one(&mut *tx)
    .await
    .unwrap_or(None);

    if let Some(ts) = max_received {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        tx.commit().await?;
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

    let base =
        "SELECT m.id, m.thread_id, m.subject, m.from_addr, m.from_name, \
                m.has_attachments, m.preview_text, m.flags, m.date, m.size_bytes \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
           AND mb.folder_name = $3";

    // Keyset pagination: resolve the anchor row's received_at + id so we can
    // use a (received_at, id) composite cursor. before_id gives the "next page"
    // (older messages); after_id gives the "previous page" (newer messages).
    let rows: Vec<MessageListItem> = if let Some(cursor_id) = params.before_id.or(params.after_id) {
        let is_before = params.before_id.is_some();

        let anchor: Option<(time::OffsetDateTime, Uuid)> = sqlx::query_as(
            "SELECT m.received_at, m.id \
             FROM messages m \
             JOIN mailboxes mb ON mb.id = m.mailbox_id \
             WHERE m.id = $1 AND m.tenant_id = $2 AND mb.user_id = $3 \
             LIMIT 1",
        )
        .bind(cursor_id)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_optional(&mut *tx)
        .await?;

        let (anchor_ts, anchor_id) = anchor.ok_or(MailError::MessageNotFound(cursor_id))?;

        if is_before {
            let sql = format!(
                "{base} {flag_filter} {multi_flag_filter} {unread_filter} {thread_id_filter} \
                 {from_addr_filter} {subject_filter} {cc_addr_filter} {has_attachments_filter} \
                 {size_min_filter} {size_max_filter} \
                 AND (m.received_at, m.id) < ($4, $5) \
                 ORDER BY m.received_at DESC, m.id DESC LIMIT $6"
            );
            sqlx::query_as(&sql)
                .bind(ctx.tenant_id)
                .bind(ctx.user_id)
                .bind(&folder)
                .bind(anchor_ts)
                .bind(anchor_id)
                .bind(limit)
                .fetch_all(&mut *tx)
                .await?
        } else {
            let sql = format!(
                "{base} {flag_filter} {multi_flag_filter} {unread_filter} {thread_id_filter} \
                 {from_addr_filter} {subject_filter} {cc_addr_filter} {has_attachments_filter} \
                 {size_min_filter} {size_max_filter} \
                 AND (m.received_at, m.id) > ($4, $5) \
                 ORDER BY m.received_at ASC, m.id ASC LIMIT $6"
            );
            let mut rows: Vec<MessageListItem> = sqlx::query_as(&sql)
                .bind(ctx.tenant_id)
                .bind(ctx.user_id)
                .bind(&folder)
                .bind(anchor_ts)
                .bind(anchor_id)
                .bind(limit)
                .fetch_all(&mut *tx)
                .await?;
            rows.reverse();
            rows
        }
    } else {
        // Legacy offset pagination.
        let offset = params.page.unwrap_or(0) * limit;
        let order = if params.sort.as_deref().map(|s| s.eq_ignore_ascii_case("asc")).unwrap_or(false) {
            "ASC"
        } else {
            "DESC"
        };
        let sql = format!(
            "{base} {flag_filter} {multi_flag_filter} {unread_filter} {thread_id_filter} \
             {from_addr_filter} {subject_filter} {cc_addr_filter} {has_attachments_filter} \
             {size_min_filter} {size_max_filter} \
             ORDER BY m.received_at {order}, m.id {order} LIMIT $4 OFFSET $5"
        );
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .bind(&folder)
            .bind(limit)
            .bind(offset)
            .fetch_all(&mut *tx)
            .await?
    };
    let count_sql = format!(
        "SELECT COUNT(*) FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
           AND mb.folder_name = $3 \
         {flag_filter} {multi_flag_filter} {unread_filter} {thread_id_filter} \
         {from_addr_filter} {subject_filter} {cc_addr_filter} {has_attachments_filter} \
         {size_min_filter} {size_max_filter}"
    );
    let total: i64 = sqlx::query_scalar(&count_sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .bind(&folder)
        .fetch_one(&mut *tx)
        .await
        .unwrap_or(0);

    tx.commit().await?;

    let mut resp = (
        StatusCode::OK,
        [(header::HeaderName::from_static("x-total-count"), total.to_string())],
        Json(rows),
    ).into_response();
    if let Some(ts) = max_received {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

/// GET /api/v1/mail/messages/:id — mark as Seen + return detail
/// GET /api/v1/mail/messages/:id — mark as Seen + return detail.
/// Returns ETag derived from received_at (immutable) + id. Responds 304 if If-None-Match or If-Modified-Since matches.
async fn get_message(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    req_headers:  axum::http::HeaderMap,
) -> Result<Response> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let msg: Option<MessageDetail> = sqlx::query_as(
        r#"
        SELECT m.id, m.mailbox_id, m.subject, m.from_addr, m.from_name,
               m.to_addrs, m.cc_addrs, m.bcc_addrs, m.reply_to, m.message_id, m.in_reply_to,
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

    let etag = format!("\"{}-{}\"", msg.received_at.unix_timestamp(), msg.id);
    let last_modified = msg.received_at
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap_or_default();

    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            tx.commit().await?;
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = time::OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if msg.received_at <= ims_dt {
                    tx.commit().await?;
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }

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

    Ok((
        StatusCode::OK,
        [
            (header::ETAG,          etag),
            (header::LAST_MODIFIED, last_modified),
        ],
        Json(msg),
    ).into_response())
}


/// GET /api/v1/mail/messages/:id/raw — download RFC 2822 bytes.
///
/// Returns `Content-Type: message/rfc822` and
/// `Content-Disposition: attachment; filename="message.eml"`.
/// ETag = `"{size_bytes}-{id}"` (immutable after delivery). Responds 304 if If-None-Match matches.
/// Fetches raw bytes from S3 or local filesystem via `body_path`.
/// Returns 404 if the message is not found or 502 if the body store is unavailable.
async fn get_message_raw(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    req_headers:  axum::http::HeaderMap,
) -> Result<Response> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row: Option<(String, Option<String>, i32)> = sqlx::query_as(
        r#"SELECT m.body_path, m.message_id, m.size_bytes
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

    let (body_path, message_id, size_bytes) = row.ok_or(MailError::MessageNotFound(id))?;

    let etag = format!("\"{}-{}\"", size_bytes, id);
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }

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
            (header::ETAG,                etag),
        ],
        Body::from(bytes),
    ).into_response())
}

/// HEAD /api/v1/mail/messages/:id/raw — check existence and get Content-Length without body download.
async fn head_message_raw(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    req_headers:  axum::http::HeaderMap,
) -> Result<Response> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row: Option<(String, i32)> = sqlx::query_as(
        r#"SELECT m.body_path, m.size_bytes
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

    let (_body_path, size_bytes) = row.ok_or(MailError::MessageNotFound(id))?;

    let etag = format!("\"{}-{}\"", size_bytes, id);
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE,   "message/rfc822".to_string()),
            (header::CONTENT_LENGTH, size_bytes.to_string()),
            (header::ETAG,           etag),
        ],
        Body::empty(),
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

/// GET /api/v1/mail/threads?folder=&limit=&offset= — list distinct threads with per-thread stats.
///
/// Returns threads visible to the authenticated user, ordered by `last_received_at DESC`
/// (most recently active first). Each entry includes `thread_id`, `message_count`,
/// `unread_count`, `subject` (from the first message), `from_addrs` (unique senders),
/// `last_received_at`, and `has_attachments` (any in thread). Sprint #625.
#[derive(Debug, Deserialize)]
struct ListThreadsParams {
    folder: Option<String>,
    limit:  Option<i64>,
    offset: Option<i64>,
}

async fn list_threads(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Query(params): Query<ListThreadsParams>,
) -> Result<Json<serde_json::Value>> {
    let limit  = params.limit.unwrap_or(50).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);

    let folder_filter = params.folder.as_deref()
        .map(|f| format!("AND mb.folder_name = '{}'", f.replace('\'', "''")))
        .unwrap_or_default();

    let sql = format!(
        "SELECT \
            m.thread_id, \
            COUNT(*)::BIGINT AS message_count, \
            COUNT(*) FILTER (WHERE NOT (m.flags @> ARRAY['\\\\Seen']))::BIGINT AS unread_count, \
            MIN(m.subject) AS subject, \
            MAX(m.received_at) AS last_received_at, \
            BOOL_OR(m.has_attachments) AS has_attachments \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
           AND m.thread_id IS NOT NULL \
           {folder_filter} \
         GROUP BY m.thread_id \
         ORDER BY last_received_at DESC \
         LIMIT $3 OFFSET $4"
    );

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let rows = sqlx::query(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;

    use sqlx::Row as _;
    let threads: Vec<serde_json::Value> = rows.iter().map(|r| {
        let thread_id:       Uuid                   = r.get("thread_id");
        let message_count:   i64                    = r.get("message_count");
        let unread_count:    i64                    = r.get("unread_count");
        let subject:         Option<String>         = r.try_get("subject").ok().flatten();
        let last_received_at: OffsetDateTime        = r.get("last_received_at");
        let has_attachments: bool                   = r.get("has_attachments");
        serde_json::json!({
            "thread_id":       thread_id,
            "message_count":   message_count,
            "unread_count":    unread_count,
            "subject":         subject,
            "last_received_at": last_received_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
            "has_attachments": has_attachments,
        })
    }).collect();

    Ok(Json(serde_json::json!({
        "threads": threads,
        "limit":   limit,
        "offset":  offset,
    })))
}

/// GET /api/v1/mail/threads/:thread_id — list all messages in thread ordered ASC.
/// Returns ETag derived from MAX(received_at) of thread messages. Responds 304 if If-None-Match matches.
async fn list_thread(
    State(state):    State<AppState>,
    ctx:             RequestCtx,
    Path(thread_id): Path<Uuid>,
    req_headers:     axum::http::HeaderMap,
) -> Result<Response> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    // Derive ETag from MAX(received_at) so it changes whenever the thread gains a new message.
    let max_ts: Option<time::OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(m.received_at) FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.thread_id = $1 AND m.tenant_id = $2 AND mb.tenant_id = $2 AND mb.user_id = $3",
    )
    .bind(thread_id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap_or(None);

    let etag = max_ts
        .map(|ts| format!("\"{}-{}\"", ts.unix_timestamp(), thread_id))
        .unwrap_or_else(|| format!("\"0-{}\"", thread_id));

    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            tx.commit().await?;
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    if let Some(ts) = max_ts {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        tx.commit().await?;
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

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

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.thread_id = $1 AND m.tenant_id = $2 AND mb.tenant_id = $2 AND mb.user_id = $3",
    )
    .bind(thread_id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap_or(0);

    tx.commit().await?;

    let mut resp = (
        StatusCode::OK,
        [
            (header::HeaderName::from_static("x-total-count"), total.to_string()),
            (header::ETAG,                                     etag),
        ],
        Json(rows),
    ).into_response();
    if let Some(ts) = max_ts {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

/// GET /api/v1/mail/messages/stats/threads?folder=
///
/// Aggregate thread stats for the user: total distinct threads, threads with at
/// least one unread message, total messages, and per-folder breakdown.
/// Useful for badge counts and "unread threads" indicators without listing threads.
/// Complements #625 (list_threads). Sprint #630.
#[derive(Debug, Deserialize)]
struct ThreadStatsParams {
    folder: Option<String>,
}

async fn thread_stats(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Query(params): Query<ThreadStatsParams>,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let folder_filter = params.folder.as_deref()
        .map(|f| format!("AND mb.folder_name = '{}'", f.replace('\'', "''")))
        .unwrap_or_default();

    let sql = format!(
        "SELECT \
            COUNT(DISTINCT m.thread_id)::BIGINT AS total_threads, \
            COUNT(DISTINCT CASE WHEN NOT (m.flags @> ARRAY['\\\\Seen']) THEN m.thread_id END)::BIGINT AS unread_threads, \
            COUNT(*)::BIGINT AS total_messages \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
           AND m.thread_id IS NOT NULL \
           {folder_filter}"
    );

    let row: (i64, i64, i64) = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;

    let (total_threads, unread_threads, total_messages) = row;

    let mut resp = serde_json::json!({
        "total_threads":   total_threads,
        "unread_threads":  unread_threads,
        "read_threads":    total_threads - unread_threads,
        "total_messages":  total_messages,
    });
    if let Some(folder) = params.folder {
        resp["folder"] = serde_json::Value::String(folder);
    }
    Ok(Json(resp))
}

/// GET /api/v1/mail/messages/:id/thread — thread da mensagem dado o message ID.
///
/// Alias conveniente pra `GET /threads/:thread_id` sem precisar conhecer o thread_id
/// de antemão. Busca o thread_id da mensagem e retorna todas as mensagens do thread
/// em ordem cronológica. 404 se a mensagem não pertence ao tenant/user.
/// Response: `{thread_id, messages: [MessageListItem]}`. Sprint #607.
async fn get_message_thread(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let thread_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT m.thread_id FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.id = $1 AND m.tenant_id = $2 AND mb.tenant_id = $2 AND mb.user_id = $3 \
         LIMIT 1",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();

    let thread_id = thread_id.ok_or(MailError::MessageNotFound(id))?;

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

    Ok(Json(serde_json::json!({
        "thread_id": thread_id,
        "messages":  rows,
    })))
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
) -> Result<Json<MessageDetail>> {
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

    let msg: Option<MessageDetail> = sqlx::query_as(
        r#"
        SELECT m.id, m.mailbox_id, m.subject, m.from_addr, m.from_name,
               m.to_addrs, m.cc_addrs, m.bcc_addrs, m.reply_to, m.message_id, m.in_reply_to,
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

    tx.commit().await?;
    msg.map(Json).ok_or(MailError::MessageNotFound(id))
}

/// GET /api/v1/mail/messages/:id/flags — list flags without marking Seen.
/// ETag = sorted flags joined; Last-Modified = received_at (immutable delivery timestamp).
async fn get_message_flags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    req_headers:  axum::http::HeaderMap,
) -> Result<Response> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row: Option<(Vec<String>, OffsetDateTime)> = sqlx::query_as(
        "SELECT m.flags, m.received_at \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.id = $1 AND m.tenant_id = $2 AND mb.tenant_id = $2 AND mb.user_id = $3 \
         LIMIT 1",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    let (mut flags, received_at) = row.ok_or(MailError::MessageNotFound(id))?;
    flags.sort_unstable();
    let etag = format!("\"{}\"", flags.join(","));
    let last_modified = received_at
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap_or_default();

    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if received_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }

    Ok((
        StatusCode::OK,
        [
            (header::ETAG,          etag),
            (header::LAST_MODIFIED, last_modified),
        ],
        Json(flags),
    ).into_response())
}

/// PATCH /api/v1/mail/messages/:id/flags
async fn update_flags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<FlagRequest>,
) -> Result<Response> {
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

    let (flags,): (Vec<String>,) = sqlx::query_as(
        "SELECT flags FROM messages \
         WHERE id = $1 AND tenant_id = $2 \
           AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $3 AND tenant_id = $2) \
         LIMIT 1",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(MailError::MessageNotFound(id))?;

    tx.commit().await?;

    Ok((StatusCode::OK, Json(flags)).into_response())
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
    MarkRead {
        ids: Vec<Uuid>,
    },
    MarkUnread {
        ids: Vec<Uuid>,
    },
}

#[derive(Debug, Serialize)]
struct BulkResult {
    affected: u64,
}

#[derive(Debug, Deserialize)]
struct BulkFlagRequest {
    pub ids:    Vec<Uuid>,
    pub add:    Vec<String>,
    pub remove: Vec<String>,
}

/// POST /api/v1/mail/messages/bulk
///
/// Apply one action to a set of messages atomically.
/// `{"action":"delete","ids":[…]}` — hard-delete messages
/// `{"action":"flag","ids":[…],"add":["\\Seen"],"remove":[]}` — update flags
/// `{"action":"move","ids":[…],"folder":"Trash"}` — move to folder
/// `{"action":"mark_read","ids":[…]}` — add `\Seen` to all
/// `{"action":"mark_unread","ids":[…]}` — remove `\Seen` from all
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
        BulkRequest::MarkRead { ids } => {
            let res = sqlx::query(
                "UPDATE messages \
                 SET flags = array(SELECT DISTINCT unnest(flags || ARRAY['\\Seen']::text[])) \
                 WHERE id = ANY($1) AND tenant_id = $2 \
                   AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $3 AND tenant_id = $2)",
            )
            .bind(ids)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .execute(&mut *tx)
            .await?;
            res.rows_affected()
        }
        BulkRequest::MarkUnread { ids } => {
            let res = sqlx::query(
                "UPDATE messages \
                 SET flags = array(SELECT unnest(flags) EXCEPT SELECT unnest(ARRAY['\\Seen']::text[])) \
                 WHERE id = ANY($1) AND tenant_id = $2 \
                   AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $3 AND tenant_id = $2)",
            )
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

/// PATCH /api/v1/mail/messages/bulk/flags
///
/// Add and/or remove flags from a set of messages in one request.
/// Body: `{"ids":[…],"add":["\\Seen","\\Flagged"],"remove":["\\Draft"]}`
/// Returns `{"affected": N}` — number of messages touched (add + remove counted separately).
async fn bulk_update_flags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<BulkFlagRequest>,
) -> Result<Json<BulkResult>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let mut affected: u64 = 0;

    if !body.add.is_empty() {
        let res = sqlx::query(
            "UPDATE messages \
             SET flags = array(SELECT DISTINCT unnest(flags || $1::text[])) \
             WHERE id = ANY($2) AND tenant_id = $3 \
               AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $4 AND tenant_id = $3)",
        )
        .bind(&body.add)
        .bind(&body.ids)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .execute(&mut *tx)
        .await?;
        affected += res.rows_affected();
    }

    if !body.remove.is_empty() {
        let res = sqlx::query(
            "UPDATE messages \
             SET flags = array(SELECT unnest(flags) EXCEPT SELECT unnest($1::text[])) \
             WHERE id = ANY($2) AND tenant_id = $3 \
               AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $4 AND tenant_id = $3)",
        )
        .bind(&body.remove)
        .bind(&body.ids)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .execute(&mut *tx)
        .await?;
        affected += res.rows_affected();
    }

    tx.commit().await?;
    Ok(Json(BulkResult { affected }))
}

#[derive(Debug, Deserialize)]
struct BulkDeleteRequest {
    ids: Vec<Uuid>,
}

/// DELETE /api/v1/mail/messages/bulk
///
/// Hard-delete a set of messages by ID in one request.
/// Body: `{"ids":[…]}`
/// Returns `{"affected": N}`.
async fn bulk_delete(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<BulkDeleteRequest>,
) -> Result<Json<BulkResult>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let deleted: Vec<(Uuid, i64)> = sqlx::query_as(
        "DELETE FROM messages \
         WHERE id = ANY($1) AND tenant_id = $2 \
           AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $3 AND tenant_id = $2) \
         RETURNING mailbox_id, uid",
    )
    .bind(&body.ids)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    // Fire-and-forget: remove deleted messages from search index.
    let search_url   = state.cfg().search_url.clone();
    let search_token = state.cfg().search_token.clone();
    if !search_url.is_empty() && !deleted.is_empty() {
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            for (mailbox_id, uid) in &deleted {
                let doc_id = format!("{mailbox_id}/{uid}");
                let mut req = client.delete(format!("{search_url}/api/v1/index/{doc_id}"));
                if !search_token.is_empty() {
                    req = req.bearer_auth(&search_token);
                }
                let _ = req.send().await;
            }
        });
    }

    Ok(Json(BulkResult { affected: deleted.len() as u64 }))
}

/// POST /api/v1/mail/messages/:id/read-receipt
///
/// Sends an MDN (Message Disposition Notification, RFC 8098) back to the
/// original sender. The caller's address is used as the Final-Recipient and
/// Reporting-UA. Idempotent: sending multiple times just re-notifies.
///
/// Returns 204 on success, 404 if the message is not found, 422 if the
/// message has no Return-Receipt-To / Reply-To / From to send to, or 502 if
/// the SMTP relay is unreachable.
async fn send_read_receipt(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<StatusCode> {
    // Fetch the message; scope the tx to avoid borrow across await.
    let (from_addr_raw, orig_message_id, subject_raw, received_at_ts) = {
        let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
        let row: Option<(Option<String>, Option<String>, Option<String>, OffsetDateTime)> = sqlx::query_as(
            r#"SELECT m.from_addr, m.message_id, m.subject, m.received_at
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
        row.ok_or(MailError::MessageNotFound(id))?
    };

    // Who to notify: the From address of the original message.
    let recipient_str = from_addr_raw
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| MailError::BadRequest(
            "message has no From address to send MDN to".into(),
        ))?;
    let to_addr: Address = recipient_str
        .parse()
        .map_err(|_| MailError::BadRequest(format!("invalid from address: {recipient_str}")))?;

    // Caller's address is the MDN sender — fetch from users table.
    let caller_addr_str: String = {
        let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
        let email: Option<String> = sqlx::query_scalar(
            "SELECT email FROM users WHERE tenant_id = $1 AND id = $2 LIMIT 1",
        )
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        email.ok_or(MailError::Forbidden)?
    };
    let from_addr: Address = caller_addr_str
        .parse()
        .map_err(|_| MailError::BadRequest("caller email address is invalid".into()))?;

    // Build RFC 8098 MDN body.
    // Part 1: human-readable notification.
    let subject_display = subject_raw.as_deref().unwrap_or("(no subject)");
    let human_part = format!(
        "This is a Message Disposition Notification for: {subject_display}\r\n\
         The message was displayed at {}.\r\n",
        received_at_ts
            .format(&time::format_description::well_known::Rfc2822)
            .unwrap_or_else(|_| "unknown".into()),
    );

    // Part 2: machine-readable message/disposition-notification
    let orig_mid_line = orig_message_id
        .as_deref()
        .map(|mid| format!("Original-Message-ID: {mid}\r\n"))
        .unwrap_or_default();
    let mdn_part = format!(
        "Reporting-UA: expresso-mail; Expresso\r\n\
         Final-Recipient: rfc822; {caller_addr_str}\r\n\
         {orig_mid_line}\
         Disposition: manual-action/MDN-sent-manually; displayed\r\n"
    );

    // Assemble raw RFC 2822 message manually to avoid lettre multipart/report
    // content-type limitation (lettre uses multipart/mixed or alternative).
    let boundary = format!("mdn_{}", id.simple());
    let mdn_subject = format!("Read: {subject_display}");
    let raw_msg = format!(
        "From: {caller_addr_str}\r\n\
         To: {recipient_str}\r\n\
         Subject: {mdn_subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/report; report-type=disposition-notification;\r\n\
         \tboundary=\"{boundary}\"\r\n\
         \r\n\
         --{boundary}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         {human_part}\r\n\
         --{boundary}\r\n\
         Content-Type: message/disposition-notification\r\n\
         \r\n\
         {mdn_part}\r\n\
         --{boundary}--\r\n"
    );

    let envelope = Envelope::new(Some(from_addr), vec![to_addr])
        .map_err(|e| MailError::InvalidMessage(e.to_string()))?;

    let smtp_host = &state.cfg().mail_server.relay_host;
    let smtp_port = state.cfg().mail_server.relay_port;
    let mailer = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(smtp_host)
        .port(smtp_port)
        .build();
    mailer
        .send_raw(&envelope, raw_msg.as_bytes())
        .await
        .map_err(|e| MailError::SendFailed(e.to_string()))?;

    tracing::info!(target: "audit",
        event = "mail.read_receipt",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        message_id = %id, recipient = %recipient_str);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct StatsParams {
    /// Mailbox name filter (e.g. "INBOX"). Omit for all folders.
    folder: Option<String>,
    /// RFC 3339 lower bound on received_at. Omit for all time.
    since:  Option<String>,
}

/// GET /api/v1/mail/messages/stats?folder=INBOX&since=<rfc3339>
///
/// Returns per-folder counts: total, unread (no `\Seen`), size_bytes.
/// When `folder` is given, returns a single-element list for that folder.
/// Grouped by mailbox name, ordered by total DESC.
async fn message_stats(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Query(params): Query<StatsParams>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db();
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let folder_filter = params.folder.as_deref().map(|f| {
        let esc = f.replace('\'', "''");
        format!("AND mb.name = '{esc}'")
    }).unwrap_or_default();

    let since_filter = params.since.as_deref().map(|s| {
        let esc = s.replace('\'', "''");
        format!("AND m.received_at >= '{esc}'::timestamptz")
    }).unwrap_or_default();

    let sql = format!(
        "SELECT mb.name AS folder, \
                COUNT(*) AS total, \
                COUNT(*) FILTER (WHERE NOT ('\\Seen' = ANY(m.flags))) AS unread, \
                COALESCE(SUM(m.size_bytes), 0)::BIGINT AS size_bytes \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
               {folder_filter} {since_filter} \
         GROUP BY mb.name \
         ORDER BY total DESC"
    );

    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter().map(|(folder, total, unread, size_bytes)| {
        serde_json::json!({
            "folder":     folder,
            "total":      total,
            "unread":     unread,
            "read":       total - unread,
            "size_bytes": size_bytes,
        })
    }).collect();

    Ok(Json(serde_json::json!({"folders": folders})))
}

#[derive(Debug, Deserialize)]
struct FlagStatsParams {
    /// Mailbox name filter (e.g. "INBOX"). Omit for all folders.
    folder: Option<String>,
}

/// GET /api/v1/mail/messages/stats/flags?folder=INBOX
///
/// Returns per-flag message counts for the user, across all folders or a
/// specific folder. Only flags present on at least one message are included.
/// Response: `{folder?, flags: [{flag, count}]}` ordered by count DESC.
/// Sprint #610.
async fn flag_stats(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Query(params): Query<FlagStatsParams>,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let folder_filter = if params.folder.is_some() {
        "AND mb.name = $3"
    } else {
        ""
    };

    let sql = format!(
        "SELECT f.flag, COUNT(*) AS cnt \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         CROSS JOIN LATERAL unnest(m.flags) AS f(flag) \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
               {folder_filter} \
         GROUP BY f.flag \
         ORDER BY cnt DESC"
    );

    let rows: Vec<(String, i64)> = if let Some(ref folder) = params.folder {
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .bind(folder)
            .fetch_all(&mut *tx)
            .await?
    } else {
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .fetch_all(&mut *tx)
            .await?
    };
    tx.commit().await?;

    let flags: Vec<serde_json::Value> = rows.into_iter().map(|(flag, count)| {
        serde_json::json!({"flag": flag, "count": count})
    }).collect();

    let mut resp = serde_json::json!({"flags": flags});
    if let Some(folder) = params.folder {
        resp["folder"] = serde_json::Value::String(folder);
    }
    Ok(Json(resp))
}

/// GET /api/v1/mail/messages/stats/senders?folder=&limit=N
///
/// Top-N senders by message count for the authenticated user. Optional `folder`
/// scopes to a specific mailbox. `limit` defaults to 20, max 200.
/// Response: `{folder?, senders: [{from_addr, count}]}` ordered by count DESC.
/// Sprint #635.
#[derive(Debug, Deserialize)]
struct SenderStatsParams {
    folder: Option<String>,
    limit:  Option<i64>,
}

async fn sender_stats(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Query(params): Query<SenderStatsParams>,
) -> Result<Json<serde_json::Value>> {
    let limit = params.limit.unwrap_or(20).min(200).max(1);
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let folder_filter = if params.folder.is_some() {
        "AND mb.name = $4"
    } else {
        ""
    };

    let sql = format!(
        "SELECT m.from_addr, COUNT(*)::BIGINT AS cnt \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
               {folder_filter} \
         GROUP BY m.from_addr \
         ORDER BY cnt DESC \
         LIMIT $3"
    );

    let rows: Vec<(String, i64)> = if let Some(ref folder) = params.folder {
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .bind(limit)
            .bind(folder)
            .fetch_all(&mut *tx)
            .await?
    } else {
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await?
    };
    tx.commit().await?;

    let senders: Vec<serde_json::Value> = rows.into_iter().map(|(from_addr, count)| {
        serde_json::json!({"from_addr": from_addr, "count": count})
    }).collect();

    let mut resp = serde_json::json!({"senders": senders});
    if let Some(folder) = params.folder {
        resp["folder"] = serde_json::Value::String(folder);
    }
    Ok(Json(resp))
}

/// GET /api/v1/mail/messages/stats/size?folder=
///
/// Message size stats for the authenticated user: avg, min, max, and total bytes.
/// Optional `folder` scopes to a specific mailbox. Useful for storage dashboards.
/// Response: `{folder?, total_messages, avg_bytes, min_bytes, max_bytes, total_bytes}`.
/// All size fields are `null` when total_messages = 0. Sprint #640.
#[derive(Debug, Deserialize)]
struct SizeStatsParams {
    folder: Option<String>,
}

async fn size_stats(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Query(params): Query<SizeStatsParams>,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let folder_filter = if params.folder.is_some() {
        "AND mb.name = $3"
    } else {
        ""
    };

    let sql = format!(
        "SELECT \
            COUNT(*)::BIGINT                     AS total_messages, \
            AVG(m.size_bytes)                    AS avg_bytes, \
            MIN(m.size_bytes)::BIGINT            AS min_bytes, \
            MAX(m.size_bytes)::BIGINT            AS max_bytes, \
            COALESCE(SUM(m.size_bytes), 0)::BIGINT AS total_bytes \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
               {folder_filter}"
    );

    let row: (i64, Option<f64>, Option<i64>, Option<i64>, i64) = if let Some(ref folder) = params.folder {
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .bind(folder)
            .fetch_one(&mut *tx)
            .await?
    } else {
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .fetch_one(&mut *tx)
            .await?
    };
    tx.commit().await?;

    let (total_messages, avg_bytes, min_bytes, max_bytes, total_bytes) = row;

    let mut resp = serde_json::json!({
        "total_messages": total_messages,
        "avg_bytes":      avg_bytes,
        "min_bytes":      min_bytes,
        "max_bytes":      max_bytes,
        "total_bytes":    total_bytes,
    });
    if let Some(folder) = params.folder {
        resp["folder"] = serde_json::Value::String(folder);
    }
    Ok(Json(resp))
}

/// GET /api/v1/mail/messages/stats/attachments?folder=
///
/// Attachment stats for the authenticated user: how many messages have attachments,
/// their combined size, and the ratio vs total. Optional `folder` scopes to a mailbox.
/// Response: `{folder?, total_messages, with_attachments, without_attachments,
///             size_bytes_with_attachments, size_bytes_without_attachments}`. Sprint #643.
#[derive(Debug, Deserialize)]
struct AttachmentStatsParams {
    folder: Option<String>,
}

/// GET /api/v1/mail/messages/stats/received-by-day?since=&until=&folder=
///
/// Volume de mensagens recebidas por dia no range `[since, until)`.
/// Retorna `{days: [{day, count}]}` ordenado ASC. `since`/`until` são RFC 3339
/// opcionais; sem eles cobre todo o histórico. `folder` opcional restringe a
/// uma mailbox. Útil pra timeline de volume de recebimento. Sprint #648.
#[derive(Debug, Deserialize)]
struct ReceivedByDayParams {
    #[serde(default, with = "time::serde::rfc3339::option")]
    since:  Option<time::OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    until:  Option<time::OffsetDateTime>,
    folder: Option<String>,
}

async fn received_by_day_stats(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Query(params): Query<ReceivedByDayParams>,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let folder_filter = if params.folder.is_some() {
        "AND mb.name = $5"
    } else {
        ""
    };

    let sql = format!(
        "SELECT \
            to_char(date_trunc('day', m.received_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            COUNT(*)::BIGINT AS count \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
           AND ($3::timestamptz IS NULL OR m.received_at >= $3) \
           AND ($4::timestamptz IS NULL OR m.received_at <  $4) \
               {folder_filter} \
         GROUP BY day \
         ORDER BY day ASC"
    );

    let rows: Vec<(String, i64)> = if let Some(ref folder) = params.folder {
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .bind(params.since)
            .bind(params.until)
            .bind(folder)
            .fetch_all(&mut *tx)
            .await?
    } else {
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .bind(params.since)
            .bind(params.until)
            .fetch_all(&mut *tx)
            .await?
    };
    tx.commit().await?;

    let days: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, count)| serde_json::json!({"day": day, "count": count}))
        .collect();
    let mut resp = serde_json::json!({"days": days});
    if let Some(folder) = params.folder {
        resp["folder"] = serde_json::Value::String(folder);
    }
    Ok(Json(resp))
}

/// GET /api/v1/mail/messages/stats/threads-by-day?since=&until=
///
/// Timeline de threads iniciadas por dia: conta threads distintas cujo `MIN(received_at)`
/// cai dentro do range `[since, until)`. Retorna `{days:[{day,count}]}` ASC.
/// `since`/`until` RFC 3339 opcionais — sem eles cobre todo o histórico.
/// Complementa `received-by-day` (#648) que conta mensagens; aqui a unidade é thread.
/// Sprint #653.
async fn threads_by_day_stats(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Query(params): Query<ReceivedByDayParams>,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    // Subquery computes MIN(received_at) per thread; outer query groups by day.
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', first_received AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            COUNT(*)::BIGINT AS count \
         FROM ( \
             SELECT m.thread_id, MIN(m.received_at) AS first_received \
               FROM messages m \
               JOIN mailboxes mb ON mb.id = m.mailbox_id \
              WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
                AND m.thread_id IS NOT NULL \
              GROUP BY m.thread_id \
         ) threads \
         WHERE ($3::timestamptz IS NULL OR first_received >= $3) \
           AND ($4::timestamptz IS NULL OR first_received <  $4) \
         GROUP BY day \
         ORDER BY day ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(params.since)
    .bind(params.until)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let days: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, count)| serde_json::json!({"day": day, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"days": days})))
}

async fn attachment_stats(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Query(params): Query<AttachmentStatsParams>,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let folder_filter = if params.folder.is_some() {
        "AND mb.name = $3"
    } else {
        ""
    };

    let sql = format!(
        "SELECT \
            COUNT(*)::BIGINT                                                                AS total_messages, \
            COUNT(*) FILTER (WHERE m.has_attachments = TRUE)::BIGINT                       AS with_attachments, \
            COUNT(*) FILTER (WHERE m.has_attachments IS DISTINCT FROM TRUE)::BIGINT        AS without_attachments, \
            COALESCE(SUM(m.size_bytes) FILTER (WHERE m.has_attachments = TRUE), 0)::BIGINT \
                                                                                           AS size_with, \
            COALESCE(SUM(m.size_bytes) FILTER (WHERE m.has_attachments IS DISTINCT FROM TRUE), 0)::BIGINT \
                                                                                           AS size_without \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
               {folder_filter}"
    );

    let row: (i64, i64, i64, i64, i64) = if let Some(ref folder) = params.folder {
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .bind(folder)
            .fetch_one(&mut *tx)
            .await?
    } else {
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .fetch_one(&mut *tx)
            .await?
    };
    tx.commit().await?;

    let (total_messages, with_attachments, without_attachments, size_with, size_without) = row;

    let mut resp = serde_json::json!({
        "total_messages":                 total_messages,
        "with_attachments":               with_attachments,
        "without_attachments":            without_attachments,
        "size_bytes_with_attachments":    size_with,
        "size_bytes_without_attachments": size_without,
    });
    if let Some(folder) = params.folder {
        resp["folder"] = serde_json::Value::String(folder);
    }
    Ok(Json(resp))
}

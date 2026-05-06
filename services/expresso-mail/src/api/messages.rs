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
        .route("/mail/messages/stats/threads-by-day",      get(threads_by_day_stats))
        .route("/mail/messages/stats/unread-by-folder",    get(unread_by_folder_stats))
        .route("/mail/messages/stats/size-by-folder",      get(size_by_folder_stats))
        .route("/mail/messages/stats/attachments-by-folder", get(attachments_by_folder_stats))
        .route("/mail/messages/stats/flags-by-folder",        get(flags_by_folder_stats))
        .route("/mail/messages/stats/received-by-folder",      get(received_by_folder_stats))
        .route("/mail/messages/stats/threads-by-folder",       get(threads_by_folder_stats))
        .route("/mail/messages/stats/senders-by-folder",       get(senders_by_folder_stats))
        .route("/mail/messages/stats/date-by-folder",           get(date_by_folder_stats))
        .route("/mail/messages/stats/cc-by-folder",              get(cc_by_folder_stats))
        .route("/mail/messages/stats/bcc-by-folder",             get(bcc_by_folder_stats))
        .route("/mail/messages/stats/reply-rate-by-folder",      get(reply_rate_by_folder_stats))
        .route("/mail/messages/stats/to-count-by-folder",        get(to_count_by_folder_stats))
        .route("/mail/messages/stats/subject-length-by-folder",  get(subject_length_by_folder_stats))
        .route("/mail/messages/stats/preview-length-by-folder",  get(preview_length_by_folder_stats))
        .route("/mail/messages/stats/has-date-by-folder",         get(has_date_by_folder_stats))
        .route("/mail/messages/stats/from-domain-by-folder",      get(from_domain_by_folder_stats))
        .route("/mail/messages/stats/in-reply-to-by-folder",      get(in_reply_to_by_folder_stats))
        .route("/mail/messages/stats/message-id-coverage",         get(message_id_coverage_stats))
        .route("/mail/messages/stats/body-size-by-folder",          get(body_size_by_folder_stats))
        .route("/mail/messages/stats/reply-to-by-folder",           get(reply_to_by_folder_stats))
        .route("/mail/messages/stats/thread-depth-by-folder",        get(thread_depth_by_folder_stats))
        .route("/mail/messages/stats/flags-summary",                  get(flags_summary_stats))
        .route("/mail/messages/stats/size-distribution",              get(size_distribution_stats))
        .route("/mail/messages/stats/oldest-newest-by-folder",        get(oldest_newest_by_folder_stats))
        .route("/mail/messages/stats/references-count-by-folder",  get(references_count_by_folder_stats))
        .route("/mail/messages/stats/to-count-distribution",        get(to_count_distribution_stats))
        .route("/mail/messages/stats/avg-recipients-by-folder",    get(avg_recipients_by_folder_stats))
        .route("/mail/messages/stats/first-message-by-folder",     get(first_message_by_folder_stats))
        .route("/mail/messages/stats/attachment-size-by-folder",   get(attachment_size_by_folder_stats))
        .route("/mail/messages/stats/read-ratio-by-folder",        get(read_ratio_by_folder_stats))
        .route("/mail/messages/stats/subject-word-count-by-folder", get(subject_word_count_by_folder_stats))
        .route("/mail/messages/stats/cc-count-distribution",       get(cc_count_distribution_stats))
        .route("/mail/messages/stats/flagged-by-folder",           get(flagged_by_folder_stats))
        .route("/mail/messages/stats/bcc-count-distribution",      get(bcc_count_distribution_stats))
        .route("/mail/messages/stats/priority-by-folder",          get(priority_by_folder_stats))
        .route("/mail/messages/stats/importance-by-folder",        get(importance_by_folder_stats))
        .route("/mail/messages/stats/sensitivity-by-folder",       get(sensitivity_by_folder_stats))
        .route("/mail/messages/stats/list-id-by-folder",           get(list_id_by_folder_stats))
        .route("/mail/messages/stats/keywords-by-folder",          get(keywords_by_folder_stats))
        .route("/mail/messages/stats/inboxed-vs-sent-by-day",      get(inboxed_vs_sent_by_day_stats))
        .route("/mail/messages/stats/auto-replied-by-folder",      get(auto_replied_by_folder_stats))
        .route("/mail/messages/stats/x-mailer-by-folder",          get(x_mailer_by_folder_stats))
        .route("/mail/messages/stats/content-type-by-folder",      get(content_type_by_folder_stats))
        .route("/mail/messages/stats/disposition-by-folder",       get(disposition_by_folder_stats))
        .route("/mail/messages/stats/organization-by-folder",      get(organization_by_folder_stats))
        .route("/mail/messages/stats/from-addr-length-by-folder",  get(from_addr_length_by_folder_stats))
        .route("/mail/messages/stats/subject-entropy",             get(subject_entropy_stats))
        .route("/mail/messages/stats/from-domain-entropy",        get(from_domain_entropy_stats))
        .route("/mail/messages/stats/has-preview-by-folder",      get(has_preview_by_folder_stats))
        .route("/mail/messages/stats/thread-age-by-folder",       get(thread_age_by_folder_stats))
        .route("/mail/messages/stats/size-entropy",                get(size_entropy_stats))
        .route("/mail/messages/stats/attachment-count-distribution", get(attachment_count_distribution_stats))
        .route("/mail/messages/stats/avg-size-by-weekday",        get(avg_size_by_weekday_stats))
        .route("/mail/messages/stats/flagged-count-by-folder",    get(flagged_count_by_folder_stats))
        .route("/mail/messages/stats/recent-by-folder",           get(recent_by_folder_stats))
        .route("/mail/messages/stats/unread-rate-by-folder",      get(unread_rate_by_folder_stats))
        .route("/mail/messages/stats/received-by-weekday",        get(received_by_weekday_stats))
        .route("/mail/messages/stats/to-addrs-per-message",       get(to_addrs_per_message_stats))
        .route("/mail/messages/stats/subject-re-fwd-by-folder",   get(subject_re_fwd_stats))
        .route("/mail/messages/stats/sender-domain-by-weekday",   get(sender_domain_by_weekday_stats))
        .route("/mail/messages/stats/thread-count-by-weekday",    get(thread_count_by_weekday_stats))
        .route("/mail/messages/stats/to-addrs-domain",            get(to_addrs_domain_stats))
        .route("/mail/messages/stats/msg-id-length-by-folder",    get(msg_id_length_stats))
        .route("/mail/messages/stats/from-addr-count",            get(from_addr_count_stats))
        .route("/mail/messages/stats/bcc-domain",                 get(bcc_domain_stats))
        .route("/mail/messages/stats/sender-coverage-by-folder",  get(sender_coverage_by_folder_stats))
        .route("/mail/messages/stats/reply-chain-depth",          get(reply_chain_depth_stats))
        .route("/mail/messages/stats/has-reply-to-by-folder",     get(has_reply_to_by_folder_stats))
        .route("/mail/messages/stats/has-cc-by-weekday",          get(has_cc_by_weekday_stats))
        .route("/mail/messages/stats/cc-count",                   get(cc_count_stats))
        .route("/mail/messages/stats/in-reply-to-depth-by-folder", get(in_reply_to_depth_by_folder_stats))
        .route("/mail/messages/stats/subject-word-count",         get(subject_word_count_stats))
        .route("/mail/messages/stats/to-addrs-count-by-folder",   get(to_addrs_count_by_folder_stats))
        .route("/mail/messages/stats/from-addr-by-weekday",       get(from_addr_by_weekday_stats))
        .route("/mail/messages/stats/size-by-weekday",            get(size_by_weekday_stats))
        .route("/mail/messages/stats/has-attachments-by-weekday", get(has_attachments_by_weekday_stats))
        .route("/mail/messages/stats/unread-by-weekday",    get(unread_by_weekday_stats))
        .route("/mail/messages/stats/flagged-by-folder",    get(flagged_by_folder_stats))
        .route("/mail/messages/stats/size-percentile",      get(size_percentile_stats))
        .route("/mail/messages/stats/date-range-by-folder", get(date_range_by_folder_stats))
        .route("/mail/messages/stats/received-by-hour",     get(received_by_hour_stats))
        .route("/mail/messages/stats/to-domain",             get(to_domain_stats))
        .route("/mail/messages/stats/age-by-folder",         get(age_by_folder_stats))
        .route("/mail/messages/stats/flagged-rate-by-folder", get(flagged_rate_by_folder_stats))
        .route("/mail/messages/stats/body-size-by-weekday",      get(body_size_by_weekday_stats))
        .route("/mail/messages/stats/received-by-month",         get(received_by_month_stats))
        .route("/mail/messages/stats/sent-by-month",             get(sent_by_month_stats))
        .route("/mail/messages/stats/read-by-month",             get(read_by_month_stats))
        .route("/mail/messages/stats/starred-by-month",          get(starred_by_month_stats))
        .route("/mail/messages/stats/body-size-by-month",        get(body_size_by_month_stats))
        .route("/mail/messages/stats/attachment-by-month",       get(attachment_by_month_stats))
        .route("/mail/messages/stats/flagged-by-month",          get(flagged_by_month_stats))
        .route("/mail/messages/stats/preview-length-by-weekday", get(preview_length_by_weekday_stats))
        .route("/mail/messages/stats/subject-length-by-weekday", get(subject_length_by_weekday_stats))
        .route("/mail/messages/stats/to-count-by-weekday",       get(to_count_by_weekday_stats))
        .route("/mail/messages/stats/bcc-count-by-weekday",  get(bcc_count_by_weekday_stats))
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

/// GET /api/v1/mail/messages/stats/attachments-by-folder — with/without attachments por mailbox.
///
/// Agrega `has_attachments` por `mb.name` retornando
/// `{folders:[{folder,with_attachments,without_attachments,size_bytes}]}`
/// ordenado por `with_attachments DESC`. `size_bytes` = total de mensagens com anexo.
/// LEFT JOIN para incluir pastas vazias com zeros. Sprint #668.
async fn attachments_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(*) FILTER (WHERE m.has_attachments = TRUE)::BIGINT AS with_attachments, \
            COUNT(*) FILTER (WHERE m.has_attachments IS DISTINCT FROM TRUE)::BIGINT AS without_attachments, \
            COALESCE(SUM(m.size_bytes) FILTER (WHERE m.has_attachments = TRUE), 0)::BIGINT AS size_bytes \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY with_attachments DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, with_attachments, without_attachments, size_bytes)| serde_json::json!({
            "folder":             folder,
            "with_attachments":   with_attachments,
            "without_attachments": without_attachments,
            "size_bytes":         size_bytes,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/flags-by-folder — LATERAL unnest flags por (folder, flag).
///
/// Agrupa via `CROSS JOIN LATERAL unnest(m.flags)` por `(mb.name, flag)` e retorna
/// `{rows:[{folder,flag,count}]}` ordenado `(folder ASC, count DESC)`. Complementa
/// flag_stats (#610) com breakdown por mailbox. Sprint #673.
async fn flags_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT mb.name AS folder, f.flag, COUNT(*)::BIGINT AS count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
           CROSS JOIN LATERAL unnest(m.flags) AS f(flag) \
          WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name, f.flag \
          ORDER BY mb.name ASC, count DESC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, flag, count)| serde_json::json!({"folder": folder, "flag": flag, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/mail/messages/stats/received-by-folder — volume total de mensagens por mailbox.
///
/// COUNT(*) por `mb.name` ordenado por `total DESC`. LEFT JOIN para incluir pastas vazias.
/// Retorna `{folders:[{folder,total}]}`. Visão de volume sem breakdown temporal.
/// Complementa received-by-day (#648) com perspectiva por mailbox. Sprint #678.
async fn received_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT mb.name AS folder, COUNT(m.id)::BIGINT AS total \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY total DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total)| serde_json::json!({"folder": folder, "total": total}))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/size-by-folder — total, avg e max size_bytes por mailbox.
///
/// Agrega `size_bytes` por `mb.name` e retorna `{folders:[{folder,total_bytes,avg_bytes,max_bytes}]}`
/// ordenado por `total_bytes DESC`. `avg_bytes` e `max_bytes` são `null` para pastas vazias.
/// Complementa `stats/size` (#640) com breakdown por folder. Sprint #663.
async fn size_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<f64>, Option<i64>)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COALESCE(SUM(m.size_bytes), 0)::BIGINT AS total_bytes, \
            AVG(m.size_bytes)::FLOAT8 AS avg_bytes, \
            MAX(m.size_bytes)::BIGINT AS max_bytes \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY total_bytes DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total_bytes, avg_bytes, max_bytes)| serde_json::json!({
            "folder":      folder,
            "total_bytes": total_bytes,
            "avg_bytes":   avg_bytes,
            "max_bytes":   max_bytes,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/unread-by-folder — contagem total e não-lidos por mailbox.
///
/// Agrupa mensagens por `mb.name` e retorna `{folders:[{folder,total,unread}]}`
/// ordenado por `unread DESC`. Escopo por user+tenant via begin_tenant_tx.
/// Complementa `stats/` com visão focada somente em leitura. Sprint #658.
async fn unread_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(*)::BIGINT AS total, \
            COUNT(*) FILTER (WHERE NOT ('\\Seen' = ANY(m.flags)))::BIGINT AS unread \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY unread DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, unread)| serde_json::json!({"folder": folder, "total": total, "unread": unread}))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/threads-by-folder — COUNT DISTINCT thread_id por mailbox.
///
/// Retorna `{folders:[{folder,thread_count,unread_thread_count}]}` ordenado por
/// `thread_count DESC`. Um thread é "unread" se tiver ao menos uma mensagem sem `\Seen`.
/// Complementa `stats/threads` (#630) com breakdown por pasta. Sprint #683.
async fn threads_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(DISTINCT m.thread_id)::BIGINT AS thread_count, \
            COUNT(DISTINCT CASE WHEN NOT ('\\Seen' = ANY(m.flags)) THEN m.thread_id END)::BIGINT \
                AS unread_thread_count \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
                              AND m.thread_id IS NOT NULL \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY thread_count DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, thread_count, unread_thread_count)| serde_json::json!({
            "folder":               folder,
            "thread_count":         thread_count,
            "unread_thread_count":  unread_thread_count,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/senders-by-folder — top-20 from_addr por mailbox.
///
/// GROUP BY (mb.name, from_addr) ORDER BY count DESC; retorna os 20 maiores remetentes
/// por pasta como `{folders:[{folder,top_senders:[{from_addr,count}]}]}`.
/// Complementa `stats/senders` com breakdown por folder. Sprint #688.
async fn senders_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT mb.name AS folder, m.from_addr, COUNT(*)::BIGINT AS count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name, m.from_addr \
          ORDER BY mb.name ASC, count DESC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    // Group rows by folder, keep top-20 per folder (SQL already orders by count DESC within folder).
    let mut map: std::collections::BTreeMap<String, Vec<serde_json::Value>> =
        std::collections::BTreeMap::new();
    for (folder, from_addr, count) in rows {
        let entry = map.entry(folder).or_default();
        if entry.len() < 20 {
            entry.push(serde_json::json!({"from_addr": from_addr, "count": count}));
        }
    }

    let folders: Vec<serde_json::Value> = map.into_iter()
        .map(|(folder, top_senders)| serde_json::json!({
            "folder":      folder,
            "top_senders": top_senders,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/date-by-folder — envelope temporal por mailbox.
///
/// Retorna `{folders:[{folder,message_count,oldest_at,newest_at}]}` ordenado por
/// `message_count DESC`. MIN/MAX received_at por pasta. Útil pra saber o range temporal
/// de cada mailbox sem listar mensagens. Sprint #693.
async fn date_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<OffsetDateTime>, Option<OffsetDateTime>)> =
        sqlx::query_as(
            "SELECT \
                mb.name AS folder, \
                COUNT(m.id)::BIGINT AS message_count, \
                MIN(m.received_at) AS oldest_at, \
                MAX(m.received_at) AS newest_at \
             FROM mailboxes mb \
             LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
             WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
             GROUP BY mb.name \
             ORDER BY message_count DESC, mb.name ASC",
        )
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, message_count, oldest_at, newest_at)| serde_json::json!({
            "folder":        folder,
            "message_count": message_count,
            "oldest_at":     oldest_at,
            "newest_at":     newest_at,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/cc-by-folder — presença de CC por mailbox.
///
/// Conta mensagens com/sem destinatários CC por pasta (LEFT JOIN para incluir pastas
/// vazias). `has_cc` = `cc_addrs IS NOT NULL AND array_length(cc_addrs, 1) > 0`.
/// Retorna `{folders:[{folder,total,with_cc,without_cc}]}` ordenado por total DESC. Sprint #697.
async fn cc_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS total, \
            COUNT(m.id) FILTER (WHERE m.cc_addrs IS NOT NULL AND array_length(m.cc_addrs, 1) > 0)::BIGINT AS with_cc, \
            COUNT(m.id) FILTER (WHERE m.cc_addrs IS NULL OR array_length(m.cc_addrs, 1) IS NULL OR array_length(m.cc_addrs, 1) = 0)::BIGINT AS without_cc \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY total DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, with_cc, without_cc)| serde_json::json!({
            "folder":     folder,
            "total":      total,
            "with_cc":    with_cc,
            "without_cc": without_cc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/bcc-by-folder — presença de BCC por mailbox.
///
/// Análogo a `cc-by-folder` (#697) mas sobre `bcc_addrs`. LEFT JOIN para incluir
/// pastas vazias. Retorna `{folders:[{folder,total,with_bcc,without_bcc}]}` ordenado
/// por total DESC. Sprint #702.
async fn bcc_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS total, \
            COUNT(m.id) FILTER (WHERE m.bcc_addrs IS NOT NULL AND array_length(m.bcc_addrs, 1) > 0)::BIGINT AS with_bcc, \
            COUNT(m.id) FILTER (WHERE m.bcc_addrs IS NULL OR array_length(m.bcc_addrs, 1) IS NULL OR array_length(m.bcc_addrs, 1) = 0)::BIGINT AS without_bcc \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY total DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, with_bcc, without_bcc)| serde_json::json!({
            "folder":      folder,
            "total":       total,
            "with_bcc":    with_bcc,
            "without_bcc": without_bcc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/reply-rate-by-folder — taxa de respostas por mailbox.
///
/// `is_reply` = `in_reply_to IS NOT NULL`. LEFT JOIN para incluir pastas vazias.
/// Retorna `{folders:[{folder,total,replies,non_replies}]}` ordenado por total DESC. Sprint #707.
async fn reply_rate_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS total, \
            COUNT(m.id) FILTER (WHERE m.in_reply_to IS NOT NULL)::BIGINT AS replies, \
            COUNT(m.id) FILTER (WHERE m.in_reply_to IS NULL)::BIGINT     AS non_replies \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY total DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, replies, non_replies)| serde_json::json!({
            "folder":      folder,
            "total":       total,
            "replies":     replies,
            "non_replies": non_replies,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/to-count-by-folder — fan-out de destinatários por mailbox.
///
/// `to_addrs` é JSONB array — usa `jsonb_array_length` para contar destinatários por mensagem.
/// Retorna `avg_to` e `max_to` por pasta. LEFT JOIN para incluir pastas vazias.
/// Retorna `{folders:[{folder,message_count,avg_to,max_to}]}` ordenado por message_count DESC. Sprint #712.
async fn to_count_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<f64>, Option<i64>)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS message_count, \
            AVG(jsonb_array_length(m.to_addrs))   AS avg_to, \
            MAX(jsonb_array_length(m.to_addrs))::BIGINT AS max_to \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY message_count DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, message_count, avg_to, max_to)| serde_json::json!({
            "folder":        folder,
            "message_count": message_count,
            "avg_to":        avg_to,
            "max_to":        max_to,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /mail/messages/stats/subject-length-by-folder — avg/max LENGTH(subject) por pasta.
///
/// Indica verbosidade dos assuntos por mailbox. LEFT JOIN para incluir pastas vazias.
/// avg_subject_length é NULL quando pasta não tem mensagens.
/// Retorna `{folders:[{folder,message_count,avg_subject_length,max_subject_length}]}` por message_count DESC.
/// Sprint #717.
async fn subject_length_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<f64>, Option<i64>)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS message_count, \
            AVG(LENGTH(m.subject)) AS avg_subject_length, \
            MAX(LENGTH(m.subject))::BIGINT AS max_subject_length \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY message_count DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, message_count, avg_len, max_len)| serde_json::json!({
            "folder":              folder,
            "message_count":       message_count,
            "avg_subject_length":  avg_len,
            "max_subject_length":  max_len,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /mail/messages/stats/preview-length-by-folder — avg/max LENGTH(preview_text) por pasta.
///
/// Indica riqueza do snippet de preview por mailbox. LEFT JOIN para incluir pastas vazias.
/// avg_preview_length é NULL quando pasta não tem mensagens. Sprint #722.
async fn preview_length_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<f64>, Option<i64>)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS message_count, \
            AVG(LENGTH(m.preview_text)) AS avg_preview_length, \
            MAX(LENGTH(m.preview_text))::BIGINT AS max_preview_length \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY message_count DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, message_count, avg_len, max_len)| serde_json::json!({
            "folder":              folder,
            "message_count":       message_count,
            "avg_preview_length":  avg_len,
            "max_preview_length":  max_len,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /mail/messages/stats/has-date-by-folder — COUNT with_date/without_date por pasta.
///
/// `date` = campo Date: do envelope (pode ser NULL se ausente/inválido na ingestão).
/// LEFT JOIN para incluir pastas vazias. Sprint #727.
async fn has_date_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS total, \
            COUNT(m.id) FILTER (WHERE m.date IS NOT NULL)::BIGINT AS with_date, \
            COUNT(m.id) FILTER (WHERE m.date IS NULL)::BIGINT AS without_date \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY total DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, with_date, without_date)| serde_json::json!({
            "folder":       folder,
            "total":        total,
            "with_date":    with_date,
            "without_date": without_date,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /mail/messages/stats/from-domain-by-folder — top-20 domínios de remetente por pasta.
///
/// Extrai o domínio de `from_addr` (parte após '@') e conta mensagens por (folder, domain).
/// Retorna `{folders:[{folder,domains:[{domain,count}]}]}` ordenado por folder, count DESC.
/// Sprint #732.
async fn from_domain_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            SPLIT_PART(m.from_addr, '@', 2) AS domain, \
            COUNT(*)::BIGINT AS cnt \
         FROM mailboxes mb \
         JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
           AND m.from_addr LIKE '%@%' \
         GROUP BY mb.name, SPLIT_PART(m.from_addr, '@', 2) \
         ORDER BY mb.name ASC, cnt DESC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let mut map: std::collections::BTreeMap<String, Vec<serde_json::Value>> = std::collections::BTreeMap::new();
    for (folder, domain, cnt) in rows {
        map.entry(folder).or_default().push(serde_json::json!({"domain": domain, "count": cnt}));
    }
    for domains in map.values_mut() {
        domains.truncate(20);
    }

    let folders: Vec<serde_json::Value> = map.into_iter()
        .map(|(folder, domains)| serde_json::json!({"folder": folder, "domains": domains}))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /mail/messages/stats/in-reply-to-by-folder — mensagens com/sem in_reply_to por pasta.
///
/// Complementa reply-rate-by-folder (#707) com LEFT JOIN para incluir pastas vazias.
/// Retorna `{folders:[{folder,total,with_in_reply_to,without_in_reply_to}]}` total DESC. Sprint #737.
async fn in_reply_to_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS total, \
            COUNT(m.id) FILTER (WHERE m.in_reply_to IS NOT NULL)::BIGINT AS with_in_reply_to, \
            COUNT(m.id) FILTER (WHERE m.in_reply_to IS NULL)::BIGINT     AS without_in_reply_to \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY total DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, with_irt, without_irt)| serde_json::json!({
            "folder":             folder,
            "total":              total,
            "with_in_reply_to":   with_irt,
            "without_in_reply_to": without_irt,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /mail/messages/stats/message-id-coverage — mensagens com/sem message_id por pasta.
///
/// Cobertura do campo `message_id` (mensagens bem-formadas têm Message-ID). LEFT JOIN para pastas vazias.
/// Retorna `{folders:[{folder,total,with_message_id,without_message_id}]}` total DESC. Sprint #742.
async fn message_id_coverage_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS total, \
            COUNT(m.id) FILTER (WHERE m.message_id IS NOT NULL AND m.message_id <> '')::BIGINT AS with_message_id, \
            COUNT(m.id) FILTER (WHERE m.message_id IS NULL OR m.message_id = '')::BIGINT       AS without_message_id \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY total DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, with_mid, without_mid)| serde_json::json!({
            "folder":            folder,
            "total":             total,
            "with_message_id":   with_mid,
            "without_message_id": without_mid,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /mail/messages/stats/body-size-by-folder — avg/max size_bytes por pasta.
///
/// `size_bytes` é o tamanho raw da mensagem (raw MIME). LEFT JOIN para pastas vazias.
/// Retorna `{folders:[{folder,message_count,avg_size_bytes,max_size_bytes}]}` total DESC. Sprint #747.
async fn body_size_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<f64>, Option<i64>)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS message_count, \
            AVG(m.size_bytes::BIGINT), \
            MAX(m.size_bytes::BIGINT)::BIGINT \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY message_count DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, message_count, avg_size, max_size)| serde_json::json!({
            "folder":         folder,
            "message_count":  message_count,
            "avg_size_bytes": avg_size,
            "max_size_bytes": max_size,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /mail/messages/stats/reply-to-by-folder — mensagens com/sem Reply-To por pasta.
///
/// `reply_to IS NOT NULL AND reply_to <> ''` indica mensagens com cabeçalho Reply-To explícito.
/// LEFT JOIN para incluir pastas vazias. Retorna `{folders:[{folder,total,with_reply_to,without_reply_to}]}` total DESC. Sprint #752.
async fn reply_to_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS total, \
            COUNT(m.id) FILTER (WHERE m.reply_to IS NOT NULL AND m.reply_to <> '')::BIGINT AS with_reply_to, \
            COUNT(m.id) FILTER (WHERE m.reply_to IS NULL OR m.reply_to = '')::BIGINT       AS without_reply_to \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY total DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, with_rt, without_rt)| serde_json::json!({
            "folder":          folder,
            "total":           total,
            "with_reply_to":   with_rt,
            "without_reply_to": without_rt,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /mail/messages/stats/thread-depth-by-folder — avg/max mensagens por thread por pasta.
///
/// Calcula depth = COUNT(msgs) agrupado por thread_id, depois agrega avg/max desses depths por pasta.
/// LEFT JOIN para incluir pastas vazias. Retorna `{folders:[{folder,thread_count,avg_depth,max_depth}]}`. Sprint #757.
async fn thread_depth_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<f64>, Option<i64>)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(DISTINCT m.thread_id)::BIGINT AS thread_count, \
            AVG(thread_sizes.depth), \
            MAX(thread_sizes.depth)::BIGINT \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         LEFT JOIN ( \
             SELECT mailbox_id, thread_id, COUNT(*)::BIGINT AS depth \
               FROM messages \
              WHERE tenant_id = $1 \
              GROUP BY mailbox_id, thread_id \
         ) thread_sizes ON thread_sizes.mailbox_id = mb.id AND thread_sizes.thread_id = m.thread_id \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY thread_count DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, thread_count, avg_depth, max_depth)| serde_json::json!({
            "folder":       folder,
            "thread_count": thread_count,
            "avg_depth":    avg_depth,
            "max_depth":    max_depth,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /mail/messages/stats/flags-summary — total de cada flag cross-folder.
///
/// LATERAL unnest(flags) cross-folder, COUNT por flag. Global no tenant/user.
/// Retorna `{flags:[{flag,count}]}` count DESC. Sprint #762.
async fn flags_summary_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT flag, COUNT(*)::BIGINT AS count \
           FROM messages m, LATERAL unnest(m.flags) AS flag \
          WHERE m.tenant_id = $1 \
            AND EXISTS ( \
                SELECT 1 FROM mailboxes mb \
                 WHERE mb.id = m.mailbox_id \
                   AND mb.tenant_id = $1 \
                   AND mb.user_id   = $2 \
            ) \
          GROUP BY flag \
          ORDER BY count DESC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let flags: Vec<serde_json::Value> = rows.into_iter()
        .map(|(flag, count)| serde_json::json!({"flag": flag, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"flags": flags})))
}

/// GET /mail/messages/stats/size-distribution — histograma de tamanho cross-folder.
///
/// Buckets: <1KB / 1-10KB / 10-100KB / 100KB-1MB / >1MB por size_bytes. Sprint #767.
async fn size_distribution_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (lt_1k, k1_to_10k, k10_to_100k, k100_to_1m, gt_1m): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT \
                COUNT(*) FILTER (WHERE m.size_bytes < 1024)::BIGINT, \
                COUNT(*) FILTER (WHERE m.size_bytes >= 1024       AND m.size_bytes < 10240)::BIGINT, \
                COUNT(*) FILTER (WHERE m.size_bytes >= 10240      AND m.size_bytes < 102400)::BIGINT, \
                COUNT(*) FILTER (WHERE m.size_bytes >= 102400     AND m.size_bytes < 1048576)::BIGINT, \
                COUNT(*) FILTER (WHERE m.size_bytes >= 1048576)::BIGINT \
             FROM messages m \
             JOIN mailboxes mb ON mb.id = m.mailbox_id AND mb.user_id = $2 \
            WHERE m.tenant_id = $1",
        )
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "lt_1kb":       lt_1k,
        "kb1_to_10kb":  k1_to_10k,
        "kb10_to_100kb": k10_to_100k,
        "kb100_to_1mb": k100_to_1m,
        "gt_1mb":       gt_1m,
    })))
}

/// GET /mail/messages/stats/oldest-newest-by-folder — MIN/MAX received_at por pasta.
///
/// Envelope temporal granular por pasta com contagem. LEFT JOIN para pastas vazias.
/// Retorna `{folders:[{folder,message_count,oldest,newest}]}` total DESC. Sprint #772.
async fn oldest_newest_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<time::OffsetDateTime>, Option<time::OffsetDateTime>)> =
        sqlx::query_as(
            "SELECT \
                mb.name AS folder, \
                COUNT(m.id)::BIGINT AS message_count, \
                MIN(m.received_at), \
                MAX(m.received_at) \
             FROM mailboxes mb \
             LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
             WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
             GROUP BY mb.name \
             ORDER BY message_count DESC, mb.name ASC",
        )
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, message_count, oldest, newest)| serde_json::json!({
            "folder":        folder,
            "message_count": message_count,
            "oldest":        oldest.map(|t| t.to_string()),
            "newest":        newest.map(|t| t.to_string()),
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/references-count-by-folder — with/without references por pasta.
///
/// COUNT mensagens com references_ array não-vazio vs vazio/null.
/// LEFT JOIN mailboxes para incluir pastas vazias.
/// Retorna `{folders:[{folder,message_count,with_references,without_references}]}`. Sprint #777.
async fn references_count_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS message_count, \
            COUNT(m.id) FILTER (WHERE m.references_ IS NOT NULL AND array_length(m.references_, 1) > 0)::BIGINT AS with_references, \
            COUNT(m.id) FILTER (WHERE m.references_ IS NULL     OR  array_length(m.references_, 1) IS NULL)::BIGINT AS without_references \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY message_count DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, mc, wr, wor)| serde_json::json!({
            "folder":             folder,
            "message_count":      mc,
            "with_references":    wr,
            "without_references": wor,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/to-count-distribution — histograma de destinatários (to_addrs).
///
/// Buckets: 0/1/2/3/4/5+ destinatários por jsonb_array_length(to_addrs).
/// Escopo: todas as mensagens do user (cross-folder). Sprint #782.
async fn to_count_distribution_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (b0, b1, b2, b3, b4, b5p): (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE COALESCE(jsonb_array_length(to_addrs), 0) = 0)::BIGINT AS b0, \
            COUNT(*) FILTER (WHERE jsonb_array_length(to_addrs) = 1)::BIGINT              AS b1, \
            COUNT(*) FILTER (WHERE jsonb_array_length(to_addrs) = 2)::BIGINT              AS b2, \
            COUNT(*) FILTER (WHERE jsonb_array_length(to_addrs) = 3)::BIGINT              AS b3, \
            COUNT(*) FILTER (WHERE jsonb_array_length(to_addrs) = 4)::BIGINT              AS b4, \
            COUNT(*) FILTER (WHERE jsonb_array_length(to_addrs) >= 5)::BIGINT             AS b5p \
         FROM messages m \
         WHERE m.tenant_id = $1 \
           AND EXISTS ( \
               SELECT 1 FROM mailboxes mb \
                WHERE mb.id = m.mailbox_id AND mb.tenant_id = $1 AND mb.user_id = $2 \
           )",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "recipients_0":   b0,
        "recipients_1":   b1,
        "recipients_2":   b2,
        "recipients_3":   b3,
        "recipients_4":   b4,
        "recipients_5p":  b5p,
    })))
}

/// GET /api/v1/mail/messages/stats/avg-recipients-by-folder — AVG total de destinatários (to+cc+bcc) por pasta.
///
/// AVG(to_addrs + cc_addrs + bcc_addrs) via jsonb_array_length, NULL = 0.
/// LEFT JOIN mailboxes para pastas vazias.
/// Retorna `{folders:[{folder,message_count,avg_recipients,max_recipients}]}`. Sprint #787.
async fn avg_recipients_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<f64>, Option<i64>)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS message_count, \
            AVG( \
                COALESCE(jsonb_array_length(m.to_addrs),  0) + \
                COALESCE(jsonb_array_length(m.cc_addrs),  0) + \
                COALESCE(jsonb_array_length(m.bcc_addrs), 0) \
            ) AS avg_recipients, \
            MAX( \
                COALESCE(jsonb_array_length(m.to_addrs),  0) + \
                COALESCE(jsonb_array_length(m.cc_addrs),  0) + \
                COALESCE(jsonb_array_length(m.bcc_addrs), 0) \
            )::BIGINT AS max_recipients \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY message_count DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, mc, avg, max)| serde_json::json!({
            "folder":          folder,
            "message_count":   mc,
            "avg_recipients":  avg,
            "max_recipients":  max,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/first-message-by-folder — MIN received_at por pasta.
///
/// Timestamp mais antigo de cada pasta para auditoria de criação/importação.
/// LEFT JOIN mailboxes para pastas vazias. Sprint #792.
async fn first_message_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, Option<time::OffsetDateTime>)> = sqlx::query_as(
        "SELECT mb.name AS folder, MIN(m.received_at) AS first_received \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY first_received ASC NULLS LAST, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, first)| serde_json::json!({
            "folder":         folder,
            "first_received": first.map(|t| t.to_string()),
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/attachment-size-by-folder — avg/max size_bytes de msgs com anexos.
///
/// Filtra has_attachments=true para refletir tamanho de mensagens com conteúdo binário.
/// LEFT JOIN mailboxes para incluir pastas sem anexos (count=0).
/// Retorna `{folders:[{folder,total_with_attachments,avg_bytes,max_bytes}]}`. Sprint #797.
async fn attachment_size_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<f64>, Option<i64>)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id) FILTER (WHERE m.has_attachments)::BIGINT AS total_with_attachments, \
            AVG(m.size_bytes::BIGINT) FILTER (WHERE m.has_attachments) AS avg_bytes, \
            MAX(m.size_bytes::BIGINT) FILTER (WHERE m.has_attachments) AS max_bytes \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY total_with_attachments DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, avg, max)| serde_json::json!({
            "folder":                 folder,
            "total_with_attachments": total,
            "avg_bytes":              avg,
            "max_bytes":              max,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/read-ratio-by-folder — read/unread ratio por pasta.
///
/// "Seen" flag indica leitura. Inclui pastas vazias via LEFT JOIN.
/// Retorna `{folders:[{folder,total,read,unread,read_pct}]}`. Sprint #802.
async fn read_ratio_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS total, \
            COUNT(m.id) FILTER (WHERE '\\Seen' = ANY(m.flags))::BIGINT AS read, \
            COUNT(m.id) FILTER (WHERE NOT ('\\Seen' = ANY(m.flags)))::BIGINT AS unread \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY total DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, read, unread)| {
            let read_pct = if total > 0 { read as f64 / total as f64 * 100.0 } else { 0.0 };
            serde_json::json!({
                "folder":   folder,
                "total":    total,
                "read":     read,
                "unread":   unread,
                "read_pct": read_pct,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/subject-word-count-by-folder — avg/max palavras em subject por pasta.
///
/// Conta via `array_length(regexp_split_to_array(trim(subject), '\s+'), 1)`.
/// Subjects null/vazio contam como 0. LEFT JOIN mailboxes para pastas vazias.
/// Retorna `{folders:[{folder,message_count,avg_words,max_words}]}`. Sprint #807.
async fn subject_word_count_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<f64>, Option<i64>)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS message_count, \
            AVG(CASE WHEN m.subject IS NOT NULL AND m.subject <> '' \
                THEN array_length(regexp_split_to_array(trim(m.subject), '\\s+'), 1) \
                ELSE 0 END) AS avg_words, \
            MAX(CASE WHEN m.subject IS NOT NULL AND m.subject <> '' \
                THEN array_length(regexp_split_to_array(trim(m.subject), '\\s+'), 1) \
                ELSE 0 END)::BIGINT AS max_words \
         FROM mailboxes mb \
         LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
         WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
         GROUP BY mb.name \
         ORDER BY message_count DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, mc, avg, max)| serde_json::json!({
            "folder":         folder,
            "message_count":  mc,
            "avg_words":      avg,
            "max_words":      max,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/cc-count-distribution — histograma de destinatários CC.
///
/// Buckets: 0/1/2/3/4/5+ via jsonb_array_length(cc_addrs). Cross-folder por user. Sprint #812.
async fn cc_count_distribution_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (b0, b1, b2, b3, b4, b5p): (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE COALESCE(jsonb_array_length(cc_addrs), 0) = 0)::BIGINT AS b0, \
            COUNT(*) FILTER (WHERE jsonb_array_length(cc_addrs) = 1)::BIGINT              AS b1, \
            COUNT(*) FILTER (WHERE jsonb_array_length(cc_addrs) = 2)::BIGINT              AS b2, \
            COUNT(*) FILTER (WHERE jsonb_array_length(cc_addrs) = 3)::BIGINT              AS b3, \
            COUNT(*) FILTER (WHERE jsonb_array_length(cc_addrs) = 4)::BIGINT              AS b4, \
            COUNT(*) FILTER (WHERE jsonb_array_length(cc_addrs) >= 5)::BIGINT             AS b5p \
         FROM messages m \
         WHERE m.tenant_id = $1 \
           AND EXISTS ( \
               SELECT 1 FROM mailboxes mb \
                WHERE mb.id = m.mailbox_id AND mb.tenant_id = $1 AND mb.user_id = $2 \
           )",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "cc_0":  b0,
        "cc_1":  b1,
        "cc_2":  b2,
        "cc_3":  b3,
        "cc_4":  b4,
        "cc_5p": b5p,
    })))
}

/// GET /api/v1/mail/messages/stats/flagged-by-folder — COUNT \Flagged por pasta.
///
/// with_flagged / without_flagged por mailbox. LEFT JOIN para incluir pastas sem mensagens.
/// Retorna `{folders:[{folder,with_flagged,without_flagged}]}` with_flagged DESC. Sprint #817.
async fn flagged_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    #[derive(sqlx::FromRow)]
    struct Row { folder: String, with_flagged: i64, without_flagged: i64 }

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id) FILTER (WHERE '\\Flagged' = ANY(m.flags))::BIGINT AS with_flagged, \
            COUNT(m.id) FILTER (WHERE '\\Flagged' <> ALL(COALESCE(m.flags, ARRAY[]::TEXT[])))::BIGINT AS without_flagged \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY with_flagged DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|r| serde_json::json!({
            "folder":          r.folder,
            "with_flagged":    r.with_flagged,
            "without_flagged": r.without_flagged,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/bcc-count-distribution — histograma destinatários BCC.
///
/// Buckets: 0/1/2/3/4/5+ via jsonb_array_length(bcc_addrs). Cross-folder por user. Sprint #822.
async fn bcc_count_distribution_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (b0, b1, b2, b3, b4, b5p): (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE COALESCE(jsonb_array_length(bcc_addrs), 0) = 0)::BIGINT AS b0, \
            COUNT(*) FILTER (WHERE jsonb_array_length(bcc_addrs) = 1)::BIGINT              AS b1, \
            COUNT(*) FILTER (WHERE jsonb_array_length(bcc_addrs) = 2)::BIGINT              AS b2, \
            COUNT(*) FILTER (WHERE jsonb_array_length(bcc_addrs) = 3)::BIGINT              AS b3, \
            COUNT(*) FILTER (WHERE jsonb_array_length(bcc_addrs) = 4)::BIGINT              AS b4, \
            COUNT(*) FILTER (WHERE jsonb_array_length(bcc_addrs) >= 5)::BIGINT             AS b5p \
         FROM messages m \
         WHERE m.tenant_id = $1 \
           AND EXISTS ( \
               SELECT 1 FROM mailboxes mb \
                WHERE mb.id = m.mailbox_id AND mb.tenant_id = $1 AND mb.user_id = $2 \
           )",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "bcc_0":  b0,
        "bcc_1":  b1,
        "bcc_2":  b2,
        "bcc_3":  b3,
        "bcc_4":  b4,
        "bcc_5p": b5p,
    })))
}

/// GET /api/v1/mail/messages/stats/priority-by-folder — distribuição de X-Priority por pasta.
///
/// COALESCE(priority, 'none') GROUP BY (folder, priority) count DESC.
/// priority é coluna TEXT (header X-Priority). Retorna `{folders:[{folder,rows:[{priority,count}]}]}`. Sprint #827.
async fn priority_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            m.priority, \
            COUNT(*)::BIGINT AS count \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name, m.priority \
          ORDER BY mb.name ASC, count DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let mut folder_map: std::collections::BTreeMap<String, Vec<serde_json::Value>> = std::collections::BTreeMap::new();
    for (folder, priority, count) in rows {
        folder_map.entry(folder).or_default()
            .push(serde_json::json!({"priority": priority, "count": count}));
    }
    let folders: Vec<serde_json::Value> = folder_map.into_iter()
        .map(|(folder, rows)| serde_json::json!({"folder": folder, "rows": rows}))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/importance-by-folder — distribuição de Importance header por pasta.
///
/// COALESCE(importance, 'normal') GROUP BY (folder, importance) count DESC.
/// importance é coluna TEXT (header Importance: high/normal/low). Sprint #832.
async fn importance_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            m.importance, \
            COUNT(*)::BIGINT AS count \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name, m.importance \
          ORDER BY mb.name ASC, count DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let mut folder_map: std::collections::BTreeMap<String, Vec<serde_json::Value>> = std::collections::BTreeMap::new();
    for (folder, importance, count) in rows {
        folder_map.entry(folder).or_default()
            .push(serde_json::json!({"importance": importance, "count": count}));
    }
    let folders: Vec<serde_json::Value> = folder_map.into_iter()
        .map(|(folder, rows)| serde_json::json!({"folder": folder, "rows": rows}))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/sensitivity-by-folder — distribuição de Sensitivity header por pasta.
///
/// Sensitivity: Normal/Personal/Private/Company-Confidential. GROUP BY (folder, sensitivity) count DESC. Sprint #837.
async fn sensitivity_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            m.sensitivity, \
            COUNT(*)::BIGINT AS count \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name, m.sensitivity \
          ORDER BY mb.name ASC, count DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let mut folder_map: std::collections::BTreeMap<String, Vec<serde_json::Value>> = std::collections::BTreeMap::new();
    for (folder, sensitivity, count) in rows {
        folder_map.entry(folder).or_default()
            .push(serde_json::json!({"sensitivity": sensitivity, "count": count}));
    }
    let folders: Vec<serde_json::Value> = folder_map.into_iter()
        .map(|(folder, rows)| serde_json::json!({"folder": folder, "rows": rows}))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/list-id-by-folder — mailing list (List-Id header) por pasta.
///
/// GROUP BY (folder, list_id) count DESC; list_id TEXT nullable. Sprint #842.
async fn list_id_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            m.list_id, \
            COUNT(*)::BIGINT AS count \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name, m.list_id \
          ORDER BY mb.name ASC, count DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let mut folder_map: std::collections::BTreeMap<String, Vec<serde_json::Value>> = std::collections::BTreeMap::new();
    for (folder, list_id, count) in rows {
        folder_map.entry(folder).or_default()
            .push(serde_json::json!({"list_id": list_id, "count": count}));
    }
    let folders: Vec<serde_json::Value> = folder_map.into_iter()
        .map(|(folder, rows)| serde_json::json!({"folder": folder, "rows": rows}))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/keywords-by-folder — X-Keywords header por pasta.
///
/// GROUP BY (folder, keywords) count DESC; keywords TEXT nullable. Sprint #847.
async fn keywords_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            m.keywords, \
            COUNT(*)::BIGINT AS count \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name, m.keywords \
          ORDER BY mb.name ASC, count DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let mut folder_map: std::collections::BTreeMap<String, Vec<serde_json::Value>> = std::collections::BTreeMap::new();
    for (folder, keywords, count) in rows {
        folder_map.entry(folder).or_default()
            .push(serde_json::json!({"keywords": keywords, "count": count}));
    }
    let folders: Vec<serde_json::Value> = folder_map.into_iter()
        .map(|(folder, rows)| serde_json::json!({"folder": folder, "rows": rows}))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/inboxed-vs-sent-by-day — COUNT msgs por (day, mailbox_type) ASC.
///
/// DATE_TRUNC('day', received_at) + mailbox_name classifies INBOX/Sent/outras.
/// Retorna `{rows:[{day,mailbox,count}]}`. Sprint #852.
async fn inboxed_vs_sent_by_day_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<ReceivedByDayParams>,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', m.received_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            UPPER(mb.name) AS mailbox, \
            COUNT(*)::BIGINT AS count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id AND mb.tenant_id = $1 \
          WHERE m.tenant_id = $1 AND m.user_id = $2 \
            AND ($3::timestamptz IS NULL OR m.received_at >= $3) \
            AND ($4::timestamptz IS NULL OR m.received_at <  $4) \
          GROUP BY day, mailbox \
          ORDER BY day ASC, mailbox ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id).bind(q.since).bind(q.until)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, mb, count)| serde_json::json!({"day": day, "mailbox": mb, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/mail/messages/stats/auto-replied-by-folder — Auto-Submitted header por pasta.
///
/// with_auto_submitted / without; identifica respostas automáticas. Sprint #857.
async fn auto_replied_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(*) FILTER (WHERE m.auto_submitted IS NOT NULL AND m.auto_submitted <> '')::BIGINT AS with_auto_submitted, \
            COUNT(*) FILTER (WHERE m.auto_submitted IS NULL OR m.auto_submitted = '')::BIGINT     AS without \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY with_auto_submitted DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, with_as, without)| serde_json::json!({
            "folder":             folder,
            "with_auto_submitted": with_as,
            "without":            without,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/x-mailer-by-folder — X-Mailer header por pasta.
///
/// GROUP BY (folder, x_mailer) count DESC. Sprint #862.
async fn x_mailer_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            m.x_mailer, \
            COUNT(*)::BIGINT AS count \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name, m.x_mailer \
          ORDER BY mb.name ASC, count DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let mut folder_map: std::collections::BTreeMap<String, Vec<serde_json::Value>> = std::collections::BTreeMap::new();
    for (folder, x_mailer, count) in rows {
        folder_map.entry(folder).or_default()
            .push(serde_json::json!({"x_mailer": x_mailer, "count": count}));
    }
    let folders: Vec<serde_json::Value> = folder_map.into_iter()
        .map(|(folder, rows)| serde_json::json!({"folder": folder, "rows": rows}))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/content-type-by-folder — Content-Type header por pasta.
///
/// GROUP BY (folder, content_type) count DESC. Sprint #867.
async fn content_type_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            m.content_type, \
            COUNT(*)::BIGINT AS count \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name, m.content_type \
          ORDER BY mb.name ASC, count DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let mut folder_map: std::collections::BTreeMap<String, Vec<serde_json::Value>> = std::collections::BTreeMap::new();
    for (folder, content_type, count) in rows {
        folder_map.entry(folder).or_default()
            .push(serde_json::json!({"content_type": content_type, "count": count}));
    }
    let folders: Vec<serde_json::Value> = folder_map.into_iter()
        .map(|(folder, rows)| serde_json::json!({"folder": folder, "rows": rows}))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/from-domain-entropy — Shannon H sobre domínios de remetente.
///
/// SPLIT_PART(from_addr,'@',2) → domínio; H=-Σp*log2(p). Retorna `{entropy,total_messages,distinct_domains}`. Sprint #892.
async fn from_domain_entropy_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 AND m.from_addr IS NOT NULL \
          GROUP BY NULLIF(SPLIT_PART(m.from_addr, '@', 2), '')",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let total: i64 = rows.iter().map(|(c,)| c).sum();
    let distinct = rows.len() as i64;
    if total == 0 || distinct < 2 {
        return Ok(Json(serde_json::json!({
            "entropy": serde_json::Value::Null,
            "total_messages": total,
            "distinct_domains": distinct,
        })));
    }
    let entropy: f64 = rows.iter().fold(0.0_f64, |acc, (c,)| {
        let p = *c as f64 / total as f64;
        acc - p * p.log2()
    });
    Ok(Json(serde_json::json!({
        "entropy": entropy,
        "total_messages": total,
        "distinct_domains": distinct,
    })))
}

/// GET /api/v1/mail/messages/stats/subject-entropy — Shannon H sobre subjects únicos.
///
/// Agrupa subject normalizado (LOWER+TRIM), calcula H=-Σp*log2(p). Retorna `{entropy,total_messages,distinct_subjects}`. Sprint #887.
async fn subject_entropy_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY LOWER(TRIM(COALESCE(m.subject, '')))",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let total: i64 = rows.iter().map(|(c,)| c).sum();
    let distinct = rows.len() as i64;
    if total == 0 || distinct < 2 {
        return Ok(Json(serde_json::json!({
            "entropy": serde_json::Value::Null,
            "total_messages": total,
            "distinct_subjects": distinct,
        })));
    }
    let entropy: f64 = rows.iter().fold(0.0_f64, |acc, (c,)| {
        let p = *c as f64 / total as f64;
        acc - p * p.log2()
    });
    Ok(Json(serde_json::json!({
        "entropy": entropy,
        "total_messages": total,
        "distinct_subjects": distinct,
    })))
}

/// GET /api/v1/mail/messages/stats/from-addr-length-by-folder — avg/max LENGTH(from_addr) por pasta.
///
/// Mede verbosidade dos remetentes. Retorna `{folders:[{folder,avg_length,max_length,with_from,without_from}]}`. Sprint #882.
async fn from_addr_length_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, Option<f64>, Option<i64>, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            AVG(LENGTH(m.from_addr))::FLOAT8  AS avg_length, \
            MAX(LENGTH(m.from_addr))::BIGINT  AS max_length, \
            COUNT(m.id) FILTER (WHERE m.from_addr IS NOT NULL)::BIGINT AS with_from, \
            COUNT(m.id) FILTER (WHERE m.from_addr IS NULL)::BIGINT     AS without_from \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY mb.name ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, avg, max, with_f, without_f)| serde_json::json!({
            "folder":       folder,
            "avg_length":   avg,
            "max_length":   max,
            "with_from":    with_f,
            "without_from": without_f,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/organization-by-folder — Organization header por pasta.
///
/// GROUP BY (folder, organization) count DESC. Sprint #877.
async fn organization_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            m.organization, \
            COUNT(*)::BIGINT AS count \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name, m.organization \
          ORDER BY mb.name ASC, count DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let mut folder_map: std::collections::BTreeMap<String, Vec<serde_json::Value>> = std::collections::BTreeMap::new();
    for (folder, organization, count) in rows {
        folder_map.entry(folder).or_default()
            .push(serde_json::json!({"organization": organization, "count": count}));
    }
    let folders: Vec<serde_json::Value> = folder_map.into_iter()
        .map(|(folder, rows)| serde_json::json!({"folder": folder, "rows": rows}))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/disposition-by-folder — Content-Disposition header por pasta.
///
/// GROUP BY (folder, disposition) count DESC. Sprint #872.
async fn disposition_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            m.disposition, \
            COUNT(*)::BIGINT AS count \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name, m.disposition \
          ORDER BY mb.name ASC, count DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let mut folder_map: std::collections::BTreeMap<String, Vec<serde_json::Value>> = std::collections::BTreeMap::new();
    for (folder, disposition, count) in rows {
        folder_map.entry(folder).or_default()
            .push(serde_json::json!({"disposition": disposition, "count": count}));
    }
    let folders: Vec<serde_json::Value> = folder_map.into_iter()
        .map(|(folder, rows)| serde_json::json!({"folder": folder, "rows": rows}))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/has-preview-by-folder — with/without preview_text por pasta.
///
/// LEFT JOIN; count with/without per mailbox. Retorna `{folders:[{folder,with_preview,without_preview}]}`. Sprint #897.
async fn has_preview_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id) FILTER (WHERE m.preview_text IS NOT NULL AND m.preview_text <> '')::BIGINT AS with_preview, \
            COUNT(m.id) FILTER (WHERE m.preview_text IS NULL OR m.preview_text = '')::BIGINT        AS without_preview \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY mb.name ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, with_p, without_p)| serde_json::json!({
            "folder":          folder,
            "with_preview":    with_p,
            "without_preview": without_p,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/thread-age-by-folder — avg age em dias por thread por pasta.
///
/// AVG(EXTRACT(EPOCH FROM (NOW()-MIN(received_at)))/86400) per thread_id per folder. Sprint #907.
async fn thread_age_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, Option<f64>, Option<f64>, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            AVG(thread_age_days)::FLOAT8 AS avg_thread_age_days, \
            MAX(thread_age_days)::FLOAT8 AS max_thread_age_days, \
            COUNT(DISTINCT thread_id)::BIGINT AS thread_count \
           FROM ( \
               SELECT m.mailbox_id, m.thread_id, \
                      EXTRACT(EPOCH FROM (NOW() - MIN(m.received_at))) / 86400.0 AS thread_age_days \
                 FROM messages m \
                WHERE m.tenant_id = $1 AND m.thread_id IS NOT NULL \
                GROUP BY m.mailbox_id, m.thread_id \
           ) t \
           JOIN mailboxes mb ON mb.id = t.mailbox_id \
          WHERE mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY mb.name ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, avg, max, tc)| serde_json::json!({
            "folder":               folder,
            "avg_thread_age_days":  avg,
            "max_thread_age_days":  max,
            "thread_count":         tc,
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/avg-size-by-weekday — AVG size_bytes por dia-da-semana (0=Dom).
///
/// EXTRACT(DOW FROM received_at) GROUP BY dow; ORDER BY dow ASC. Sprint #932.
async fn avg_size_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, f64, f64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM m.received_at AT TIME ZONE 'UTC')::INT AS dow, \
            AVG(m.size_bytes)::FLOAT8 AS avg_size_bytes, \
            MAX(m.size_bytes)::FLOAT8 AS max_size_bytes, \
            COUNT(*)::BIGINT          AS message_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
            AND m.received_at IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let rows_out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, avg, max, count)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "avg_size_bytes": avg, "max_size_bytes": max, "message_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": rows_out})))
}

/// GET /api/v1/mail/messages/stats/flagged-count-by-folder — Flagged + total + flagged_rate por pasta.
///
/// COUNT FILTER WHERE '\\Flagged' = ANY(flags); ORDER BY flagged DESC. Sprint #927.
async fn flagged_count_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(*) FILTER (WHERE '\\Flagged' = ANY(m.flags))::BIGINT AS flagged_count, \
            COUNT(*)::BIGINT AS total \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY flagged_count DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, flagged, total)| {
            let rate = if total > 0 { (flagged as f64 / total as f64 * 1000.0).round() / 1000.0 } else { 0.0 };
            serde_json::json!({"folder": folder, "flagged_count": flagged, "total": total, "flagged_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/recent-by-folder — COUNT msgs últimas 24h/7d/30d por pasta.
///
/// COUNT FILTER por janelas temporais; ORDER BY last_24h DESC. Sprint #922.
async fn recent_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(*) FILTER (WHERE m.received_at >= NOW() - INTERVAL '1 day')::BIGINT   AS last_24h, \
            COUNT(*) FILTER (WHERE m.received_at >= NOW() - INTERVAL '7 days')::BIGINT  AS last_7d, \
            COUNT(*) FILTER (WHERE m.received_at >= NOW() - INTERVAL '30 days')::BIGINT AS last_30d, \
            COUNT(*)::BIGINT AS total \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY last_24h DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, h24, d7, d30, total)| serde_json::json!({
            "folder": folder, "last_24h": h24, "last_7d": d7, "last_30d": d30, "total": total
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/received-by-weekday — COUNT por DOW de received_at (0=Dom).
///
/// EXTRACT(DOW FROM received_at); ORDER BY dow ASC. Sprint #952.
async fn received_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM m.received_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS message_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
            AND m.received_at IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let rows_out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "message_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": rows_out})))
}

/// GET /api/v1/mail/messages/stats/to-addrs-per-message — AVG/MAX jsonb_array_length(to_addrs) global.
///
/// Cross-folder cross-mailbox; total_messages incluso. Sprint #947.
async fn to_addrs_per_message_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (avg, max, total): (Option<f64>, Option<i64>, i64) = sqlx::query_as(
        "SELECT \
            AVG(jsonb_array_length(m.to_addrs))::FLOAT8 AS avg_to_count, \
            MAX(jsonb_array_length(m.to_addrs))::BIGINT AS max_to_count, \
            COUNT(*)::BIGINT AS total_messages \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
            AND m.to_addrs IS NOT NULL",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "avg_to_count":    avg,
        "max_to_count":    max,
        "total_messages":  total,
    })))
}

/// GET /api/v1/mail/messages/stats/subject-re-fwd-by-folder — COUNT com/sem RE:/FWD: por pasta.
///
/// ILIKE 'Re:%' OR 'Fwd:%'; ORDER BY replies DESC. Sprint #942.
async fn subject_re_fwd_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(*) FILTER (WHERE m.subject ILIKE 'Re:%')::BIGINT    AS replies, \
            COUNT(*) FILTER (WHERE m.subject ILIKE 'Fwd:%' OR m.subject ILIKE 'Fw:%')::BIGINT AS forwards, \
            COUNT(*)::BIGINT AS total \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY replies DESC, mb.name ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, replies, forwards, total)| serde_json::json!({
            "folder": folder, "replies": replies, "forwards": forwards, "total": total
        }))
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/sender-domain-by-weekday — top domínio remetente por DOW.
///
/// SPLIT_PART(from_addr,'@',2) × EXTRACT(DOW); top domínio por cada dia. Sprint #937.
async fn sender_domain_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, String, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM m.received_at AT TIME ZONE 'UTC')::INT AS dow, \
            LOWER(NULLIF(SPLIT_PART(m.from_addr, '@', 2), '')) AS domain, \
            COUNT(*)::BIGINT AS count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
            AND m.received_at IS NOT NULL \
            AND m.from_addr IS NOT NULL AND m.from_addr LIKE '%@%' \
          GROUP BY dow, domain \
          ORDER BY dow ASC, count DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let mut by_dow: std::collections::BTreeMap<i32, Vec<serde_json::Value>> = std::collections::BTreeMap::new();
    for (dow, domain, count) in rows {
        by_dow.entry(dow).or_default().push(serde_json::json!({"domain": domain, "count": count}));
    }
    let result: Vec<serde_json::Value> = by_dow.into_iter()
        .map(|(dow, domains)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "domains": domains})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /api/v1/mail/messages/stats/unread-rate-by-folder — unread/total ratio por pasta.
///
/// Retorna `{folders:[{folder,total,unread,unread_rate}]}` ORDER BY unread_rate DESC. Sprint #917.
async fn unread_rate_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(*)::BIGINT AS total, \
            COUNT(*) FILTER (WHERE NOT ('\\Seen' = ANY(m.flags)))::BIGINT AS unread \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY (COUNT(*) FILTER (WHERE NOT ('\\Seen' = ANY(m.flags)))::FLOAT8 / NULLIF(COUNT(*), 0)) DESC NULLS LAST",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let folders: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, unread)| {
            let rate = if total > 0 { (unread as f64 / total as f64 * 1000.0).round() / 1000.0 } else { 0.0 };
            serde_json::json!({"folder": folder, "total": total, "unread": unread, "unread_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"folders": folders})))
}

/// GET /api/v1/mail/messages/stats/size-entropy — Shannon H sobre 5 size buckets cross-folder.
///
/// Buckets <1KB/1-10KB/10-100KB/100KB-1MB/>1MB; H=-Σp*log2(p). Retorna `{entropy,total_messages,buckets:[]}`. Sprint #912.
async fn size_entropy_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (c0, c1, c2, c3, c4): (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE size_bytes < 1024)::BIGINT               AS lt_1kb, \
            COUNT(*) FILTER (WHERE size_bytes BETWEEN 1024 AND 10239)::BIGINT AS b_1_10kb, \
            COUNT(*) FILTER (WHERE size_bytes BETWEEN 10240 AND 102399)::BIGINT AS b_10_100kb, \
            COUNT(*) FILTER (WHERE size_bytes BETWEEN 102400 AND 1048575)::BIGINT AS b_100kb_1mb, \
            COUNT(*) FILTER (WHERE size_bytes >= 1048576)::BIGINT           AS gte_1mb \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    let counts = [c0, c1, c2, c3, c4];
    let total: i64 = counts.iter().sum();
    let labels = ["<1KB", "1-10KB", "10-100KB", "100KB-1MB", ">=1MB"];
    let buckets: Vec<serde_json::Value> = labels.iter().zip(counts.iter())
        .map(|(l, c)| serde_json::json!({"range": l, "count": c}))
        .collect();
    let entropy = if total < 2 { None } else {
        Some(counts.iter().filter(|&&c| c > 0).fold(0.0_f64, |acc, &c| {
            let p = c as f64 / total as f64;
            acc - p * p.log2()
        }))
    };
    Ok(Json(serde_json::json!({"entropy": entropy, "total_messages": total, "buckets": buckets})))
}

/// GET /api/v1/mail/messages/stats/attachment-count-distribution — histograma com/sem anexos cross-folder.
///
/// with_attachments, without_attachments, pct_with. Sprint #902.
async fn attachment_count_distribution_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (with_att, without_att): (i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE has_attachments = true)::BIGINT  AS with_attachments, \
            COUNT(*) FILTER (WHERE has_attachments = false)::BIGINT AS without_attachments \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    let total = with_att + without_att;
    let pct = if total == 0 { 0.0 } else { with_att as f64 / total as f64 * 100.0 };
    Ok(Json(serde_json::json!({
        "with_attachments":    with_att,
        "without_attachments": without_att,
        "total_messages":      total,
        "pct_with_attachments": pct,
    })))
}

/// GET /mail/messages/stats/from-addr-count — COUNT DISTINCT from_addr por pasta.
///
/// LEFT JOIN para incluir pastas sem mensagens. Sprint #957.
async fn from_addr_count_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS total_messages, \
            COUNT(DISTINCT m.from_addr)::BIGINT AS distinct_senders \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id \
              AND m.tenant_id = $1 \
          WHERE mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY distinct_senders DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, distinct)| serde_json::json!({
            "folder": folder,
            "total_messages": total,
            "distinct_senders": distinct,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/msg-id-length-by-folder — avg/max LENGTH(message_id) por pasta.
///
/// Só mensagens com message_id IS NOT NULL. Sprint #962.
async fn msg_id_length_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, Option<f64>, Option<i64>, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            AVG(LENGTH(m.message_id))             AS avg_length, \
            MAX(LENGTH(m.message_id))::BIGINT     AS max_length, \
            COUNT(*) FILTER (WHERE m.message_id IS NOT NULL)::BIGINT AS with_message_id, \
            COUNT(*)::BIGINT                      AS total_messages \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id \
              AND m.tenant_id = $1 \
          WHERE mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY avg_length DESC NULLS LAST",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, avg, max, with_id, total)| serde_json::json!({
            "folder": folder,
            "avg_length": avg,
            "max_length": max,
            "with_message_id": with_id,
            "total_messages": total,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/to-addrs-domain — top domínios de destinatários em to_addrs jsonb.
///
/// Extrai SPLIT_PART de cada elemento de to_addrs via jsonb_array_elements_text. Sprint #967.
async fn to_addrs_domain_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            LOWER(NULLIF(SPLIT_PART(addr.val, '@', 2), '')) AS domain, \
            COUNT(*)::BIGINT AS occurrence_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
           JOIN LATERAL jsonb_array_elements_text(m.to_addrs) AS addr(val) ON true \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY domain \
          ORDER BY occurrence_count DESC \
          LIMIT 20",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(domain, count)| serde_json::json!({"domain": domain, "occurrence_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/thread-count-by-weekday — COUNT DISTINCT thread_id por DOW.
///
/// EXTRACT(DOW FROM received_at): 0=Dom..6=Sáb. Sprint #972.
async fn thread_count_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM received_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS message_count, \
            COUNT(DISTINCT thread_id)::BIGINT AS distinct_threads \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
            AND received_at IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, msg_count, thread_count)| {
            let name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": name, "message_count": msg_count, "distinct_threads": thread_count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/has-reply-to-by-folder — with/without reply_to por pasta.
///
/// LEFT JOIN pastas vazias. Sprint #977.
async fn has_reply_to_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id) FILTER (WHERE m.reply_to IS NOT NULL AND m.reply_to <> '')::BIGINT AS with_reply_to, \
            COUNT(m.id) FILTER (WHERE m.reply_to IS NULL OR m.reply_to = '')::BIGINT AS without_reply_to, \
            COUNT(m.id)::BIGINT AS total_messages \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id \
              AND m.tenant_id = $1 \
          WHERE mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY with_reply_to DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, with_rt, without_rt, total)| serde_json::json!({
            "folder": folder,
            "with_reply_to": with_rt,
            "without_reply_to": without_rt,
            "total_messages": total,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/reply-chain-depth — avg/max msgs com in_reply_to por pasta.
///
/// COUNT reply msgs (in_reply_to IS NOT NULL) + ratio por pasta. Sprint #982.
async fn reply_chain_depth_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, f64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id) FILTER (WHERE m.in_reply_to IS NOT NULL)::BIGINT AS reply_count, \
            COUNT(m.id)::BIGINT AS total_messages, \
            CASE WHEN COUNT(m.id) > 0 \
                 THEN COUNT(m.id) FILTER (WHERE m.in_reply_to IS NOT NULL)::FLOAT8 / COUNT(m.id) \
                 ELSE 0.0 END AS reply_ratio \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id \
              AND m.tenant_id = $1 \
          WHERE mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY reply_ratio DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, reply_count, total, ratio)| serde_json::json!({
            "folder": folder,
            "reply_count": reply_count,
            "total_messages": total,
            "reply_ratio": ratio,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/sender-coverage-by-folder — DISTINCT from_addr / total por pasta.
///
/// sender_coverage_pct = distinct_senders / total * 100. Sprint #987.
async fn sender_coverage_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, f64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(DISTINCT m.from_addr)::BIGINT AS distinct_senders, \
            COUNT(m.id)::BIGINT AS total_messages, \
            CASE WHEN COUNT(m.id) > 0 \
                 THEN COUNT(DISTINCT m.from_addr)::FLOAT8 / COUNT(m.id) * 100.0 \
                 ELSE 0.0 END AS sender_coverage_pct \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id \
              AND m.tenant_id = $1 \
          WHERE mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY distinct_senders DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, distinct, total, pct)| serde_json::json!({
            "folder": folder,
            "distinct_senders": distinct,
            "total_messages": total,
            "sender_coverage_pct": pct,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/bcc-domain — top domínios em bcc_addrs jsonb.
///
/// LATERAL jsonb_array_elements_text(bcc_addrs) + SPLIT_PART('@'). Sprint #992.
async fn bcc_domain_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            LOWER(NULLIF(SPLIT_PART(addr.val, '@', 2), '')) AS domain, \
            COUNT(*)::BIGINT AS occurrence_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
           JOIN LATERAL jsonb_array_elements_text(m.bcc_addrs) AS addr(val) ON true \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY domain \
          ORDER BY occurrence_count DESC \
          LIMIT 20",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(domain, count)| serde_json::json!({"domain": domain, "occurrence_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/has-cc-by-weekday — with/without cc_addrs per DOW. Sprint #1003.
async fn has_cc_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM m.received_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*) FILTER (WHERE m.cc_addrs IS NOT NULL AND jsonb_array_length(m.cc_addrs) > 0)::BIGINT AS with_cc, \
            COUNT(*) FILTER (WHERE m.cc_addrs IS NULL OR jsonb_array_length(m.cc_addrs) = 0)::BIGINT  AS without_cc \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, with_cc, without_cc)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "with_cc": with_cc, "without_cc": without_cc})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/cc-count — distribution of cc_addrs array length. Sprint #1004.
async fn cc_count_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (total, with_cc, avg_cc, max_cc): (i64, i64, Option<f64>, Option<i64>) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT AS total_messages, \
            COUNT(*) FILTER (WHERE cc_addrs IS NOT NULL AND jsonb_array_length(cc_addrs) > 0)::BIGINT AS with_cc, \
            AVG(jsonb_array_length(cc_addrs)) FILTER (WHERE cc_addrs IS NOT NULL AND jsonb_array_length(cc_addrs) > 0) AS avg_cc_count, \
            MAX(jsonb_array_length(cc_addrs))::BIGINT FILTER (WHERE cc_addrs IS NOT NULL) AS max_cc_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "total_messages": total,
        "with_cc": with_cc,
        "without_cc": total - with_cc,
        "avg_cc_count": avg_cc,
        "max_cc_count": max_cc,
    })))
}

/// GET /mail/messages/stats/in-reply-to-depth-by-folder — fraction of messages that are replies per folder. Sprint #1005.
async fn in_reply_to_depth_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, Option<f64>)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(*) FILTER (WHERE m.in_reply_to IS NOT NULL)::BIGINT AS reply_count, \
            COUNT(m.id)::BIGINT AS total_messages, \
            CASE WHEN COUNT(m.id) > 0 \
                 THEN COUNT(*) FILTER (WHERE m.in_reply_to IS NOT NULL)::FLOAT8 / COUNT(m.id) \
                 ELSE NULL END AS reply_depth_ratio \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY reply_depth_ratio DESC NULLS LAST",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, replies, total, ratio)| serde_json::json!({
            "folder": folder,
            "reply_count": replies,
            "total_messages": total,
            "reply_depth_ratio": ratio,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/subject-word-count — avg/max word count in subject field. Sprint #1006.
async fn subject_word_count_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (total, with_subject, avg_words, max_words): (i64, i64, Option<f64>, Option<i64>) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT AS total_messages, \
            COUNT(*) FILTER (WHERE subject IS NOT NULL AND TRIM(subject) <> '')::BIGINT AS with_subject, \
            AVG(array_length(regexp_split_to_array(TRIM(subject), '\\s+'), 1)) \
                FILTER (WHERE subject IS NOT NULL AND TRIM(subject) <> '') AS avg_words, \
            MAX(array_length(regexp_split_to_array(TRIM(subject), '\\s+'), 1))::BIGINT \
                FILTER (WHERE subject IS NOT NULL AND TRIM(subject) <> '') AS max_words \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "total_messages": total,
        "with_subject": with_subject,
        "avg_subject_words": avg_words,
        "max_subject_words": max_words,
    })))
}

/// GET /mail/messages/stats/to-addrs-count-by-folder — avg to_addrs array length por folder. Sprint #1023.
async fn to_addrs_count_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<f64>, Option<i64>)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS total_messages, \
            AVG(jsonb_array_length(m.to_addrs)) FILTER (WHERE m.to_addrs IS NOT NULL AND jsonb_array_length(m.to_addrs) > 0) AS avg_to_count, \
            MAX(jsonb_array_length(m.to_addrs))::BIGINT FILTER (WHERE m.to_addrs IS NOT NULL) AS max_to_count \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY avg_to_count DESC NULLS LAST",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, avg, max)| serde_json::json!({
            "folder": folder,
            "total_messages": total,
            "avg_to_count": avg,
            "max_to_count": max,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/from-addr-by-weekday — top from_addr × DOW. Sprint #1024.
async fn from_addr_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, String, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM m.received_at AT TIME ZONE 'UTC')::INT AS dow, \
            m.from_addr, \
            COUNT(*)::BIGINT AS message_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY dow, m.from_addr \
          ORDER BY dow ASC, message_count DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, from_addr, count)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "from_addr": from_addr, "message_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/size-by-weekday — avg/total size_bytes por DOW. Sprint #1025.
async fn size_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64, i64, Option<f64>)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM m.received_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*)::BIGINT AS message_count, \
            COALESCE(SUM(m.size_bytes), 0)::BIGINT AS total_bytes, \
            AVG(m.size_bytes) AS avg_bytes \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, count, total, avg)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "message_count": count, "total_bytes": total, "avg_bytes": avg})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/has-attachments-by-weekday — with/without attachments por DOW. Sprint #1026.
async fn has_attachments_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM m.received_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*) FILTER (WHERE m.has_attachments = TRUE)::BIGINT  AS with_attachments, \
            COUNT(*) FILTER (WHERE m.has_attachments = FALSE OR m.has_attachments IS NULL)::BIGINT AS without_attachments \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, with_att, without_att)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "with_attachments": with_att, "without_attachments": without_att})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/unread-by-weekday — COUNT unread messages por DOW. Sprint #1043.
async fn unread_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM m.received_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*) FILTER (WHERE m.is_read = FALSE OR m.is_read IS NULL)::BIGINT AS unread_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, unread, total)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "unread_count": unread, "total_count": total})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/flagged-by-folder — COUNT flagged messages por folder. Sprint #1044.
async fn flagged_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(*) FILTER (WHERE m.flags IS NOT NULL AND m.flags @> '[\"\\\\Flagged\"]'::jsonb)::BIGINT AS flagged_count, \
            COUNT(m.id)::BIGINT AS total_count \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY flagged_count DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, flagged, total)| serde_json::json!({"folder": folder, "flagged_count": flagged, "total_count": total}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/size-percentile — P25/P50/P75/P90 de size_bytes globais. Sprint #1045.
async fn size_percentile_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (p25, p50, p75, p90, total): (Option<i64>, Option<i64>, Option<i64>, Option<i64>, i64) = sqlx::query_as(
        "SELECT \
            PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY m.size_bytes)::BIGINT AS p25, \
            PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY m.size_bytes)::BIGINT AS p50, \
            PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY m.size_bytes)::BIGINT AS p75, \
            PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY m.size_bytes)::BIGINT AS p90, \
            COUNT(*)::BIGINT AS total_messages \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 AND m.size_bytes IS NOT NULL",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "total_messages": total,
        "p25_bytes": p25,
        "p50_bytes": p50,
        "p75_bytes": p75,
        "p90_bytes": p90,
    })))
}

/// GET /mail/messages/stats/date-range-by-folder — MIN/MAX received_at por folder. Sprint #1046.
async fn date_range_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS total_messages, \
            to_char(MIN(m.received_at) AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS oldest_message, \
            to_char(MAX(m.received_at) AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS newest_message \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY total_messages DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, oldest, newest)| serde_json::json!({
            "folder": folder,
            "total_messages": total,
            "oldest_message": oldest,
            "newest_message": newest,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/received-by-hour — COUNT mensagens por hora do dia (0-23). Sprint #1063.
async fn received_by_hour_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM m.received_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(m.id)::BIGINT AS message_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 AND m.received_at IS NOT NULL \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, c)| serde_json::json!({"hour_of_day": h, "message_count": c}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/to-domain — top domínios em to_addrs jsonb (SPLIT_PART '@'). Sprint #1064.
async fn to_domain_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT \
            LOWER(SPLIT_PART(addr.val #>> '{}', '@', 2)) AS domain, \
            COUNT(*)::BIGINT AS address_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
           JOIN LATERAL jsonb_array_elements(m.to_addrs) AS addr(val) ON true \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
            AND m.to_addrs IS NOT NULL AND jsonb_array_length(m.to_addrs) > 0 \
          GROUP BY domain \
          HAVING LOWER(SPLIT_PART(addr.val #>> '{}', '@', 2)) <> '' \
          ORDER BY address_count DESC \
          LIMIT 30",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(domain, count)| serde_json::json!({"domain": domain, "address_count": count}))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/age-by-folder — avg age em dias (NOW() - received_at) por pasta. Sprint #1065.
async fn age_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, Option<f64>, Option<f64>)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS total_messages, \
            AVG(EXTRACT(EPOCH FROM (NOW() - m.received_at)) / 86400.0) AS avg_age_days, \
            MAX(EXTRACT(EPOCH FROM (NOW() - m.received_at)) / 86400.0) AS max_age_days \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 AND m.received_at IS NOT NULL \
          WHERE mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY avg_age_days DESC NULLS LAST",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, avg_age, max_age)| serde_json::json!({
            "folder": folder,
            "total_messages": total,
            "avg_age_days": avg_age,
            "max_age_days": max_age,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/flagged-rate-by-folder — ratio flagged/total por pasta. Sprint #1066.
async fn flagged_rate_by_folder_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT \
            mb.name AS folder, \
            COUNT(m.id)::BIGINT AS total_messages, \
            COUNT(m.id) FILTER (WHERE m.flags @> '[\"\\\\Flagged\"]'::jsonb)::BIGINT AS flagged_count \
           FROM mailboxes mb \
           LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1 \
          WHERE mb.user_id = $2 \
          GROUP BY mb.name \
          ORDER BY flagged_count DESC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(folder, total, flagged)| {
            let rate = if total > 0 { flagged as f64 / total as f64 } else { 0.0 };
            serde_json::json!({
                "folder": folder,
                "total_messages": total,
                "flagged_count": flagged,
                "flagged_rate": rate,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/flagged-by-month — COUNT mensagens flagged por mês (1–12). Sprint #1112.
async fn flagged_by_month_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM m.received_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*) FILTER (WHERE m.flagged = true)::BIGINT AS flagged_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, flagged, total)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            let rate = if total > 0 { flagged as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"month": month, "month_name": month_name, "flagged_count": flagged, "total_count": total, "flagged_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/starred-by-month — COUNT mensagens starred por mês (1–12). Sprint #1127.
async fn starred_by_month_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM m.received_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*) FILTER (WHERE m.starred = true)::BIGINT AS starred_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, starred, total)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            let rate = if total > 0 { starred as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"month": month, "month_name": month_name, "starred_count": starred, "total_count": total, "starred_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/body-size-by-month — AVG/SUM body_size por mês (1–12). Sprint #1132.
async fn body_size_by_month_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, f64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM m.received_at AT TIME ZONE 'UTC')::INT AS month, \
            AVG(m.body_size)::FLOAT8 AS avg_body_size, \
            COALESCE(SUM(m.body_size), 0)::BIGINT AS total_body_size \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
            AND m.body_size IS NOT NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, avg, total)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "avg_body_size": avg, "total_body_size": total})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/read-by-month — COUNT mensagens lidas por mês (1–12). Sprint #1117.
async fn read_by_month_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM m.received_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*) FILTER (WHERE m.read = true)::BIGINT AS read_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, read, total)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            let rate = if total > 0 { read as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"month": month, "month_name": month_name, "read_count": read, "total_count": total, "read_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/attachment-by-month — COUNT mensagens com anexo por mês (1–12). Sprint #1122.
async fn attachment_by_month_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM m.received_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*) FILTER (WHERE m.has_attachments = true)::BIGINT AS with_attachment_count, \
            COUNT(*)::BIGINT AS total_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, with_att, total)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            let rate = if total > 0 { with_att as f64 / total as f64 } else { 0.0 };
            serde_json::json!({"month": month, "month_name": month_name, "with_attachment_count": with_att, "total_count": total, "attachment_rate": rate})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/sent-by-month — COUNT mensagens com sent_at por mês (1–12). Sprint #1107.
async fn sent_by_month_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM m.sent_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*)::BIGINT AS message_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
            AND m.sent_at IS NOT NULL \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, count)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "message_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/received-by-month — COUNT mensagens recebidas por mês (1–12). Sprint #1102.
async fn received_by_month_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MONTH FROM m.received_at AT TIME ZONE 'UTC')::INT AS month, \
            COUNT(*)::BIGINT AS message_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY month \
          ORDER BY month ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const MONTH_NAMES: [&str; 12] = [
        "January","February","March","April","May","June",
        "July","August","September","October","November","December",
    ];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(month, count)| {
            let month_name = MONTH_NAMES.get((month - 1) as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"month": month, "month_name": month_name, "message_count": count})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/body-size-by-weekday — AVG/MAX body_size × DOW. Sprint #1097.
async fn body_size_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, f64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM m.received_at AT TIME ZONE 'UTC')::INT AS dow, \
            AVG(m.body_size)::FLOAT8 AS avg_body_size, \
            MAX(m.body_size)::BIGINT AS max_body_size \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
            AND m.body_size IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, avg, max)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "avg_body_size": avg, "max_body_size": max})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/preview-length-by-weekday — AVG/MAX LENGTH(preview) × DOW. Sprint #1092.
async fn preview_length_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, f64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM m.received_at AT TIME ZONE 'UTC')::INT AS dow, \
            AVG(LENGTH(m.preview))::FLOAT8 AS avg_preview_length, \
            MAX(LENGTH(m.preview))::BIGINT AS max_preview_length \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
            AND m.preview IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, avg, max)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "avg_preview_length": avg, "max_preview_length": max})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/subject-length-by-weekday — AVG/MAX LENGTH(subject) × DOW. Sprint #1087.
async fn subject_length_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, f64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM m.received_at AT TIME ZONE 'UTC')::INT AS dow, \
            AVG(LENGTH(m.subject))::FLOAT8 AS avg_subject_length, \
            MAX(LENGTH(m.subject))::BIGINT AS max_subject_length \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
            AND m.subject IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, avg, max)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "avg_subject_length": avg, "max_subject_length": max})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/to-count-by-weekday — AVG/MAX jsonb_array_length(to_addrs) × DOW. Sprint #1082.
async fn to_count_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, f64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM m.received_at AT TIME ZONE 'UTC')::INT AS dow, \
            AVG(jsonb_array_length(m.to_addrs))::FLOAT8 AS avg_to_count, \
            MAX(jsonb_array_length(m.to_addrs))::BIGINT AS max_to_count \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
            AND m.to_addrs IS NOT NULL \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, avg, max)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "avg_to_count": avg, "max_to_count": max})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

/// GET /mail/messages/stats/bcc-count-by-weekday — COUNT mensagens com bcc_addrs × DOW. Sprint #1077.
async fn bcc_count_by_weekday_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(i32, i64, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW FROM m.received_at AT TIME ZONE 'UTC')::INT AS dow, \
            COUNT(*) FILTER (WHERE m.bcc_addrs IS NOT NULL AND jsonb_array_length(m.bcc_addrs) > 0)::BIGINT AS with_bcc, \
            COUNT(*) FILTER (WHERE m.bcc_addrs IS NULL OR jsonb_array_length(m.bcc_addrs) = 0)::BIGINT  AS without_bcc \
           FROM messages m \
           JOIN mailboxes mb ON mb.id = m.mailbox_id \
          WHERE m.tenant_id = $1 AND mb.user_id = $2 \
          GROUP BY dow \
          ORDER BY dow ASC",
    )
    .bind(ctx.tenant_id).bind(ctx.user_id)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    const DAY_NAMES: [&str; 7] = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(dow, with_bcc, without_bcc)| {
            let day_name = DAY_NAMES.get(dow as usize).copied().unwrap_or("Unknown");
            serde_json::json!({"dow": dow, "day_name": day_name, "with_bcc": with_bcc, "without_bcc": without_bcc})
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": result})))
}

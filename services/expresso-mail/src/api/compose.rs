//! Send email endpoint.
//!
//! From-spoof guard: os endpoints aceitavam `from` arbitrário do cliente e
//! submetiam direto ao relay SMTP — qualquer usuário autenticado podia
//! enviar mail como qualquer outro (inclusive cross-tenant). Agora
//! `assert_from_is_authenticated_user` verifica que `req.from` bate com o
//! email do usuário autenticado (case-insensitive) — ou, para "send as", que o
//! dono daquele endereço concedeu ao caller uma delegação `SEND`
//! (`mailbox_delegations`) — antes de enviar.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use expresso_core::begin_tenant_tx;
use lettre::{
    message::{header::ContentType, Mailbox, Message, MultiPart, SinglePart},
    Address, AsyncSmtpTransport, AsyncTransport, Tokio1Executor,
};
use serde::{Deserialize, Serialize};
use serde_json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    api::context::RequestCtx,
    error::{MailError, Result},
    ingest,
    state::AppState,
};

/// Limites duros pro endpoint de envio.
///
/// Sem isso, qualquer usuário autenticado vira spam-cannon: enviar pra
/// 100k recipients num shot, body de 50 MiB, etc. — usando o relay SMTP
/// do tenant e custando reputação de IP/domínio.
///
/// 100 recipients combina to+cc+bcc; cobre listas internas legítimas
/// sem virar BCC-bomb. 1 MiB de body cobre HTML rich + assinatura;
/// anexos grandes vão por outro fluxo. 998 bytes de subject = limite
/// de linha do RFC 5322.
pub const MAX_RECIPIENTS_PER_MESSAGE: usize = 100;
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_SUBJECT_BYTES: usize = 998;

/// VCALENDAR payload cap pro send_itip. Convites reais (com participantes,
/// VALARM, recurrence rules) ficam em poucos KiB; 256 KiB cobre até
/// agendas insanas. Acima disso é abuso — ICS gigante engasga MUAs e
/// vira amplificador via mailing-list de calendário.
pub const MAX_ICS_BYTES: usize = 256 * 1024;

#[allow(clippy::doc_lazy_continuation, clippy::doc_overindented_list_items)]
/// Recipient entries beginning with this prefix name a contact group by UUID
/// (`group:<uuid>`) and are expanded to the group's member emails.
const GROUP_PREFIX: &str = "group:";

/// Expand `group:<uuid>` entries in a recipient list into member emails; plain
/// addresses pass through unchanged. A group must belong to the caller (same
/// tenant + owner) — sending to another user's group is silently treated as an
/// unknown group (no rows), never a leak. Members without an `email_primary`
/// are skipped. Deduplicates while preserving first-seen order.
async fn expand_recipients(
    state: &AppState,
    ctx: &RequestCtx,
    entries: &[String],
) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::with_capacity(entries.len());
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for entry in entries {
        match entry.strip_prefix(GROUP_PREFIX).map(str::trim) {
            Some(id_str) => {
                let group_id = Uuid::parse_str(id_str).map_err(|_| {
                    MailError::InvalidMessage(format!("invalid group id: {id_str}"))
                })?;
                for email in group_member_emails(state, ctx, group_id).await? {
                    if seen.insert(email.to_ascii_lowercase()) {
                        out.push(email);
                    }
                }
            }
            None => {
                if seen.insert(entry.to_ascii_lowercase()) {
                    out.push(entry.clone());
                }
            }
        }
    }
    Ok(out)
}

/// Fetch the member emails of a contact group owned by the caller. Tenant- and
/// owner-scoped so one user cannot expand another's group; RLS on the joined
/// tables backs the explicit `WHERE`.
async fn group_member_emails(
    state: &AppState,
    ctx: &RequestCtx,
    group_id: Uuid,
) -> Result<Vec<String>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let emails: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT c.email_primary
        FROM contact_group_members m
        JOIN contact_groups g ON g.id = m.group_id
        JOIN contacts        c ON c.id = m.contact_id
        WHERE m.tenant_id = $1
          AND m.group_id  = $2
          AND g.owner_user_id = $3
          AND c.email_primary IS NOT NULL
          AND c.email_primary <> ''
        ORDER BY c.email_primary
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(group_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(emails)
}

/// Rejeita com 403 se `claimed_from` não bater com o email do usuário
/// autenticado (case-insensitive, trim). A consulta usa `begin_tenant_tx`
/// e `WHERE tenant_id = $1 AND id = $2` — defense-in-depth contra RLS
/// NULL-bypass em `users`.
/// Authorize the `From` address. Allowed when it's the caller's own address, or
/// when it belongs to another tenant user who granted the caller a `SEND`
/// mailbox delegation ("send as"). Anything else is 403.
async fn assert_from_is_authenticated_user(
    state: &AppState,
    ctx: &RequestCtx,
    claimed_from: &str,
) -> Result<()> {
    let claimed = claimed_from.trim();
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let own: Option<String> = sqlx::query_scalar(
        r#"SELECT email FROM users
            WHERE tenant_id = $1 AND id = $2
            LIMIT 1"#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;

    if own
        .as_deref()
        .is_some_and(|e| e.trim().eq_ignore_ascii_case(claimed))
    {
        tx.commit().await?;
        return Ok(());
    }

    // "Send as": the From owner granted this caller a SEND delegation.
    let send_as: Option<(Uuid,)> = sqlx::query_as(
        r#"SELECT u.id FROM users u
             JOIN mailbox_delegations d
               ON d.tenant_id = u.tenant_id AND d.owner_id = u.id
            WHERE u.tenant_id = $1
              AND lower(u.email) = lower($2)
              AND d.delegate_id = $3
              AND d.access = 'SEND'
            LIMIT 1"#,
    )
    .bind(ctx.tenant_id)
    .bind(claimed)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    if send_as.is_some() {
        Ok(())
    } else {
        Err(MailError::Forbidden)
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/send", post(send_message))
        .route("/mail/send-with-undo", post(send_with_undo))
        .route("/mail/send-itip", post(send_itip))
        .route("/mail/messages/schedule", post(schedule_message))
        .route("/mail/messages/scheduled", get(list_scheduled))
        .route("/mail/messages/scheduled/count", get(count_scheduled))
        .route(
            "/mail/messages/scheduled/cancel-all",
            post(cancel_all_scheduled),
        )
        .route("/mail/messages/sent/count", get(count_sent))
        .route("/mail/messages/drafts/count", get(count_drafts))
        .route("/mail/messages/trash/count", get(count_trash))
        .route("/mail/messages/spam/count", get(count_spam))
        .route("/mail/messages/:id/cancel-send", post(cancel_send))
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct SendRequest {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Option<Vec<String>>,
    pub bcc: Option<Vec<String>>,
    pub subject: String,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub reply_to_id: Option<Uuid>,
}

/// POST /api/v1/mail/send
pub async fn send_message(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(req): Json<SendRequest>,
) -> Result<StatusCode> {
    validate_send_request(&req)?;
    assert_from_is_authenticated_user(&state, &ctx, &req.from).await?;

    // Expand any `group:<uuid>` recipients into their member emails before the
    // message is built; plain addresses pass through unchanged.
    let to = expand_recipients(&state, &ctx, &req.to).await?;
    let cc = expand_recipients(&state, &ctx, req.cc.as_deref().unwrap_or(&[])).await?;
    let bcc = expand_recipients(&state, &ctx, req.bcc.as_deref().unwrap_or(&[])).await?;

    let from_addr: Address = req
        .from
        .parse()
        .map_err(|_| MailError::InvalidMessage(format!("invalid from: {}", req.from)))?;

    let mut builder = Message::builder()
        .from(Mailbox::new(None, from_addr))
        .subject(&req.subject);

    for addr_str in &to {
        let a: Address = addr_str
            .parse()
            .map_err(|_| MailError::InvalidMessage(format!("invalid to: {addr_str}")))?;
        builder = builder.to(Mailbox::new(None, a));
    }
    for addr_str in &cc {
        let a: Address = addr_str
            .parse()
            .map_err(|_| MailError::InvalidMessage(format!("invalid cc: {addr_str}")))?;
        builder = builder.cc(Mailbox::new(None, a));
    }
    for addr_str in &bcc {
        let a: Address = addr_str
            .parse()
            .map_err(|_| MailError::InvalidMessage(format!("invalid bcc: {addr_str}")))?;
        builder = builder.bcc(Mailbox::new(None, a));
    }

    // Threading: if replying to a known message, resolve In-Reply-To + References.
    if let Some(orig_id) = req.reply_to_id {
        let row: Option<(Option<String>, Vec<String>)> = {
            let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
            let r = sqlx::query_as(
                r#"SELECT m.message_id, m.references_
                   FROM messages  m
                   JOIN mailboxes mb ON mb.id = m.mailbox_id
                   WHERE m.id       = $1
                     AND m.tenant_id = $2
                     AND mb.user_id  = $3
                   LIMIT 1"#,
            )
            .bind(orig_id)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .fetch_optional(&mut *tx)
            .await?;
            tx.commit().await?;
            r
        };
        if let Some((Some(mid), orig_refs)) = row {
            builder = builder.in_reply_to(mid.clone());
            // References = original References + original Message-Id (RFC 5322 §3.6.4)
            let mut refs_list = orig_refs;
            refs_list.push(mid);
            builder = builder.references(refs_list.join(" "));
        }
    }

    // Build body — prefer multipart when both variants present
    let email = match (req.body_html.as_deref(), req.body_text.as_deref()) {
        (Some(html), Some(plain)) => builder.multipart(
            MultiPart::alternative()
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_PLAIN)
                        .body(plain.to_string()),
                )
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_HTML)
                        .body(html.to_string()),
                ),
        ),
        (Some(html), None) => builder.singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_HTML)
                .body(html.to_string()),
        ),
        (None, plain_opt) => builder.singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_PLAIN)
                .body(plain_opt.unwrap_or("").to_string()),
        ),
    }
    .map_err(|e| MailError::InvalidMessage(e.to_string()))?;

    let smtp_host = &state.cfg().mail_server.relay_host;
    let smtp_port = state.cfg().mail_server.relay_port;

    let mailer = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(smtp_host)
        .port(smtp_port)
        .build();

    // Serialize once; sign if DKIM configured (signing failure is non-fatal).
    let raw = email.formatted();
    let (to_relay, dkim) = if let Some(signer) = state.dkim() {
        match signer.sign(&raw) {
            Ok(sig_header) => {
                let mut signed = Vec::with_capacity(sig_header.len() + raw.len());
                signed.extend_from_slice(sig_header.as_bytes());
                signed.extend_from_slice(&raw);
                (signed, true)
            }
            Err(e) => {
                tracing::warn!(error = %e, "DKIM sign failed — sending unsigned");
                (raw.clone(), false)
            }
        }
    } else {
        (raw.clone(), false)
    };

    let envelope = email.envelope().clone();
    mailer
        .send_raw(&envelope, &to_relay)
        .await
        .map_err(|e| MailError::SendFailed(e.to_string()))?;

    tracing::info!(from = %req.from, to = ?req.to, subject = %req.subject, dkim, "message sent");

    // Async best-effort: save to Sent folder. Failure is logged but never blocks
    // the 202 response — the message has already been relayed successfully.
    if let Err(e) = save_to_sent(&state, &ctx, &to_relay, r"\Sent").await {
        tracing::warn!(error = %e, "save-to-sent failed");
    }

    Ok(StatusCode::ACCEPTED)
}

// ─────────────────────────────────────────────────────────────────────────────
// Scheduled send: store a draft message with deliver_at; background worker sends it.

/// Min scheduling horizon: 60 s prevents accidental near-instant send while
/// still allowing fine-grained control.
const MIN_SCHEDULE_SECONDS: i64 = 60;

/// Undo-send window bounds. The client holds the message this long before it is
/// relayed; the user may `cancel-send` within the window. Capped at 30 s
/// (Gmail's max) so a forgotten send still goes out promptly.
const UNDO_MIN_SECONDS: i64 = 5;
const UNDO_MAX_SECONDS: i64 = 30;
const UNDO_DEFAULT_SECONDS: i64 = 10;

/// Build the MIME message from a SendRequest and persist it as a Draft-flagged
/// row with `deliver_at` set, returning the new message id. Shared by
/// `schedule_message` and `send_with_undo` — the only difference between them
/// is how `deliver_at` is chosen and validated by the caller.
async fn persist_for_delivery(
    state: &AppState,
    ctx: &RequestCtx,
    req: &SendRequest,
    deliver_at: OffsetDateTime,
) -> Result<Uuid> {
    let from_addr: Address = req
        .from
        .parse()
        .map_err(|_| MailError::InvalidMessage(format!("invalid from: {}", req.from)))?;

    let mut builder = Message::builder()
        .from(Mailbox::new(None, from_addr))
        .subject(&req.subject);

    for addr_str in &req.to {
        let a: Address = addr_str
            .parse()
            .map_err(|_| MailError::InvalidMessage(format!("invalid to: {addr_str}")))?;
        builder = builder.to(Mailbox::new(None, a));
    }
    for addr_str in req.cc.iter().flatten() {
        let a: Address = addr_str
            .parse()
            .map_err(|_| MailError::InvalidMessage(format!("invalid cc: {addr_str}")))?;
        builder = builder.cc(Mailbox::new(None, a));
    }
    for addr_str in req.bcc.iter().flatten() {
        let a: Address = addr_str
            .parse()
            .map_err(|_| MailError::InvalidMessage(format!("invalid bcc: {addr_str}")))?;
        builder = builder.bcc(Mailbox::new(None, a));
    }

    let email = match (req.body_html.as_deref(), req.body_text.as_deref()) {
        (Some(html), Some(plain)) => builder.multipart(
            MultiPart::alternative()
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_PLAIN)
                        .body(plain.to_string()),
                )
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_HTML)
                        .body(html.to_string()),
                ),
        ),
        (Some(html), None) => builder.singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_HTML)
                .body(html.to_string()),
        ),
        (None, plain_opt) => builder.singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_PLAIN)
                .body(plain_opt.unwrap_or("").to_string()),
        ),
    }
    .map_err(|e| MailError::InvalidMessage(e.to_string()))?;

    let raw = email.formatted();
    let body_path = ingest::write_raw_message(state, &raw)
        .await
        .map_err(|e| MailError::SendFailed(e.to_string()))?;
    let size_bytes = raw.len().min(i32::MAX as usize) as i32;
    let msg_id = Uuid::now_v7();

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let row: Option<(Uuid, i64)> = sqlx::query_as(
        "SELECT id, next_uid FROM mailboxes \
         WHERE user_id = $1 AND tenant_id = $2 AND special_use = $3 FOR UPDATE",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(r"\Drafts")
    .fetch_optional(&mut *tx)
    .await?;

    let (mbox_id, uid) = if let Some(r) = row {
        r
    } else {
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

    sqlx::query("UPDATE mailboxes SET next_uid = next_uid + 1 WHERE id = $1")
        .bind(mbox_id)
        .execute(&mut *tx)
        .await?;

    let to_json = serde_json::to_value(&req.to).unwrap_or(serde_json::Value::Array(vec![]));
    sqlx::query(
        "INSERT INTO messages \
           (id, mailbox_id, tenant_id, uid, flags, size_bytes, body_path, \
            received_at, deliver_at, from_addr, to_addrs) \
         VALUES ($1, $2, $3, $4, ARRAY[$5::text], $6, $7, now(), $8, $9, $10)",
    )
    .bind(msg_id)
    .bind(mbox_id)
    .bind(ctx.tenant_id)
    .bind(uid)
    .bind(r"\Draft")
    .bind(size_bytes)
    .bind(&body_path)
    .bind(deliver_at)
    .bind(&req.from)
    .bind(to_json)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(msg_id)
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct ScheduleRequest {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Option<Vec<String>>,
    pub bcc: Option<Vec<String>>,
    pub subject: String,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub reply_to_id: Option<Uuid>,
    /// RFC 3339 timestamp — must be at least 60 s in the future.
    #[serde(with = "time::serde::rfc3339")]
    pub deliver_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct ScheduleResp {
    pub id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub deliver_at: OffsetDateTime,
}

/// POST /api/v1/mail/messages/schedule
///
/// Stores the message as a draft with `deliver_at` set; the background
/// `scheduled_send_worker` picks it up and relays it when the time comes.
pub async fn schedule_message(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(req): Json<ScheduleRequest>,
) -> Result<(StatusCode, Json<ScheduleResp>)> {
    let send_req = SendRequest {
        from: req.from.clone(),
        to: req.to.clone(),
        cc: req.cc.clone(),
        bcc: req.bcc.clone(),
        subject: req.subject.clone(),
        body_text: req.body_text.clone(),
        body_html: req.body_html.clone(),
        reply_to_id: req.reply_to_id,
    };
    validate_send_request(&send_req)?;
    assert_from_is_authenticated_user(&state, &ctx, &req.from).await?;

    let now = OffsetDateTime::now_utc();
    if (req.deliver_at - now).whole_seconds() < MIN_SCHEDULE_SECONDS {
        return Err(MailError::InvalidMessage(format!(
            "deliver_at must be at least {MIN_SCHEDULE_SECONDS}s in the future"
        )));
    }

    let msg_id = persist_for_delivery(&state, &ctx, &send_req, req.deliver_at).await?;

    tracing::info!(target: "audit",
        event = "mail.schedule",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        msg_id = %msg_id, deliver_at = %req.deliver_at);

    Ok((
        StatusCode::CREATED,
        Json(ScheduleResp {
            id: msg_id,
            deliver_at: req.deliver_at,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct UndoSendRequest {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Option<Vec<String>>,
    pub bcc: Option<Vec<String>>,
    pub subject: String,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub reply_to_id: Option<Uuid>,
    /// Seconds to hold before relaying (clamped to [UNDO_MIN, UNDO_MAX]).
    /// Omitted → UNDO_DEFAULT_SECONDS.
    pub undo_seconds: Option<i64>,
}

/// POST /api/v1/mail/send-with-undo
///
/// Holds the message `undo_seconds` before the background worker relays it; the
/// caller may `POST /mail/messages/:id/cancel-send` within that window to abort.
/// Returns the message id and the cancellation deadline so the client can show
/// an "Undo" toast counting down to it.
pub async fn send_with_undo(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(req): Json<UndoSendRequest>,
) -> Result<(StatusCode, Json<ScheduleResp>)> {
    let send_req = SendRequest {
        from: req.from.clone(),
        to: req.to.clone(),
        cc: req.cc.clone(),
        bcc: req.bcc.clone(),
        subject: req.subject.clone(),
        body_text: req.body_text.clone(),
        body_html: req.body_html.clone(),
        reply_to_id: req.reply_to_id,
    };
    validate_send_request(&send_req)?;
    assert_from_is_authenticated_user(&state, &ctx, &req.from).await?;

    let window = req
        .undo_seconds
        .unwrap_or(UNDO_DEFAULT_SECONDS)
        .clamp(UNDO_MIN_SECONDS, UNDO_MAX_SECONDS);
    let deliver_at = OffsetDateTime::now_utc() + time::Duration::seconds(window);

    let msg_id = persist_for_delivery(&state, &ctx, &send_req, deliver_at).await?;

    tracing::info!(target: "audit",
        event = "mail.send_with_undo",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        msg_id = %msg_id, undo_seconds = window);

    Ok((
        StatusCode::CREATED,
        Json(ScheduleResp {
            id: msg_id,
            deliver_at,
        }),
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// iTIP delivery (RFC 6047): wrap an ICS body as a text/calendar MIME part with
// METHOD parameter and ship through the same SMTP relay used by /mail/send.
// Calendar service (or any client) produces the ICS — this endpoint only
// handles the MIME wrapping + relay hand-off.

use lettre::message::Attachment;

#[derive(Debug, Deserialize)]
pub struct SendItipRequest {
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    /// iTIP method: REQUEST, REPLY, CANCEL, REFRESH.
    pub method: String,
    /// Plain-text fallback body; ICS-only clients still render from the ics part.
    pub body_text: Option<String>,
    /// Raw VCALENDAR payload (CRLF-terminated per RFC 5545).
    pub ics: String,
}

pub async fn send_itip(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(req): Json<SendItipRequest>,
) -> Result<StatusCode> {
    validate_itip_request(&req)?;
    assert_from_is_authenticated_user(&state, &ctx, &req.from).await?;

    let method = req.method.trim().to_ascii_uppercase();
    match method.as_str() {
        "REQUEST" | "REPLY" | "CANCEL" | "REFRESH" => {}
        _ => {
            return Err(MailError::InvalidMessage(format!(
                "unsupported iTIP method: {method}"
            )))
        }
    }

    let from_addr: Address = req
        .from
        .parse()
        .map_err(|_| MailError::InvalidMessage(format!("invalid from: {}", req.from)))?;

    let mut builder = Message::builder()
        .from(Mailbox::new(None, from_addr))
        .subject(&req.subject);

    for addr_str in &req.to {
        let a: Address = addr_str
            .parse()
            .map_err(|_| MailError::InvalidMessage(format!("invalid to: {addr_str}")))?;
        builder = builder.to(Mailbox::new(None, a));
    }

    // RFC 6047 §2.1: text/calendar with method=<METHOD> parameter.
    let calendar_ct: ContentType = format!("text/calendar; method={method}; charset=utf-8")
        .parse()
        .map_err(|e: lettre::message::header::ContentTypeErr| {
            MailError::InvalidMessage(e.to_string())
        })?;

    // Build multipart/alternative: plain text + text/calendar. Use attachment
    // form so that MUAs that don't render inline still offer the ics as a file.
    let plain = req.body_text.unwrap_or_else(|| {
        format!("This is an iTIP {method} invitation. Your mail client should display it inline.")
    });

    let ics_part =
        Attachment::new("invite.ics".to_string()).body(req.ics.into_bytes(), calendar_ct);

    let email = builder
        .multipart(
            MultiPart::mixed()
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_PLAIN)
                        .body(plain),
                )
                .singlepart(ics_part),
        )
        .map_err(|e| MailError::InvalidMessage(e.to_string()))?;

    let smtp_host = &state.cfg().mail_server.relay_host;
    let smtp_port = state.cfg().mail_server.relay_port;

    let mailer = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(smtp_host)
        .port(smtp_port)
        .build();

    mailer
        .send(email)
        .await
        .map_err(|e| MailError::SendFailed(e.to_string()))?;

    tracing::info!(from=%req.from, to=?req.to, method=%method, "itip dispatched");
    Ok(StatusCode::ACCEPTED)
}

/// Store raw RFC 2822 bytes to the user's Sent mailbox with `\Seen` flag.
/// Returns Ok even when the mailbox doesn't exist — it is auto-created.
async fn save_to_sent(
    state: &AppState,
    ctx: &RequestCtx,
    raw: &[u8],
    special_use: &str,
) -> anyhow::Result<()> {
    let body_path = ingest::write_raw_message(state, raw).await?;
    let size_bytes = raw.len().min(i32::MAX as usize) as i32;
    let msg_id = Uuid::now_v7();

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let row: Option<(Uuid, i64)> = sqlx::query_as(
        "SELECT id, next_uid FROM mailboxes \
         WHERE user_id = $1 AND tenant_id = $2 AND special_use = $3 FOR UPDATE",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(special_use)
    .fetch_optional(&mut *tx)
    .await?;

    let (mbox_id, uid) = if let Some((mid, nu)) = row {
        (mid, nu)
    } else {
        let folder_name = match special_use {
            r"\Sent" => "Sent",
            r"\Drafts" => "Drafts",
            other => other,
        };
        let mid: Uuid = sqlx::query_scalar(
            "INSERT INTO mailboxes \
               (user_id, tenant_id, folder_name, special_use, uid_validity, next_uid, subscribed) \
             VALUES ($1, $2, $3, $4, EXTRACT(EPOCH FROM now())::BIGINT, 1, true) \
             RETURNING id",
        )
        .bind(ctx.user_id)
        .bind(ctx.tenant_id)
        .bind(folder_name)
        .bind(special_use)
        .fetch_one(&mut *tx)
        .await?;
        (mid, 1i64)
    };

    sqlx::query("UPDATE mailboxes SET next_uid = next_uid + 1 WHERE id = $1")
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
    .bind(r"\Seen")
    .bind(size_bytes)
    .bind(&body_path)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Gate aplicado em /mail/send antes do bind ao relay SMTP. Ordem dos
/// checks: subject → recipients → body. Mantém o de baixo custo (len)
/// antes do que poderia alocar (Vec scan).
fn validate_send_request(req: &SendRequest) -> Result<()> {
    if req.subject.len() > MAX_SUBJECT_BYTES {
        return Err(MailError::InvalidMessage(format!(
            "subject too large: {} bytes (max {})",
            req.subject.len(),
            MAX_SUBJECT_BYTES
        )));
    }
    if req.subject.contains('\r') || req.subject.contains('\n') {
        return Err(MailError::InvalidMessage(
            "subject must not contain CR or LF".into(),
        ));
    }
    let recipient_count =
        req.to.len() + req.cc.as_ref().map_or(0, Vec::len) + req.bcc.as_ref().map_or(0, Vec::len);
    if recipient_count == 0 {
        return Err(MailError::InvalidMessage("no recipients".into()));
    }
    if recipient_count > MAX_RECIPIENTS_PER_MESSAGE {
        return Err(MailError::InvalidMessage(format!(
            "too many recipients: {} (max {})",
            recipient_count, MAX_RECIPIENTS_PER_MESSAGE
        )));
    }
    let body_total =
        req.body_text.as_deref().map_or(0, str::len) + req.body_html.as_deref().map_or(0, str::len);
    if body_total > MAX_BODY_BYTES {
        return Err(MailError::InvalidMessage(format!(
            "body too large: {} bytes (max {})",
            body_total, MAX_BODY_BYTES
        )));
    }
    Ok(())
}

/// Gate aplicado em /mail/send-itip antes do bind ao relay. Reusa os caps
/// de subject/recipients do /mail/send + cap próprio pro VCALENDAR.
fn validate_itip_request(req: &SendItipRequest) -> Result<()> {
    if req.subject.len() > MAX_SUBJECT_BYTES {
        return Err(MailError::InvalidMessage(format!(
            "subject too large: {} bytes (max {})",
            req.subject.len(),
            MAX_SUBJECT_BYTES
        )));
    }
    if req.subject.contains('\r') || req.subject.contains('\n') {
        return Err(MailError::InvalidMessage(
            "subject must not contain CR or LF".into(),
        ));
    }
    if req.to.is_empty() {
        return Err(MailError::InvalidMessage("no recipients".into()));
    }
    if req.to.len() > MAX_RECIPIENTS_PER_MESSAGE {
        return Err(MailError::InvalidMessage(format!(
            "too many recipients: {} (max {})",
            req.to.len(),
            MAX_RECIPIENTS_PER_MESSAGE
        )));
    }
    if req.ics.len() > MAX_ICS_BYTES {
        return Err(MailError::InvalidMessage(format!(
            "ics payload too large: {} bytes (max {})",
            req.ics.len(),
            MAX_ICS_BYTES
        )));
    }
    if let Some(b) = req.body_text.as_deref() {
        if b.len() > MAX_BODY_BYTES {
            return Err(MailError::InvalidMessage(format!(
                "body too large: {} bytes (max {})",
                b.len(),
                MAX_BODY_BYTES
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct ScheduledItem {
    id: Uuid,
    subject: Option<String>,
    from_addr: Option<String>,
    to_addrs: serde_json::Value,
    #[serde(with = "time::serde::rfc3339")]
    deliver_at: OffsetDateTime,
    size_bytes: i32,
}

/// GET /api/v1/mail/messages/scheduled — lista mensagens agendadas (deliver_at no
/// futuro) do usuário autenticado, ordenadas por horário de entrega (sprint #419).
#[allow(clippy::type_complexity)]
async fn list_scheduled(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<Vec<ScheduledItem>>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<(
        Uuid,
        Option<String>,
        Option<String>,
        serde_json::Value,
        OffsetDateTime,
        i32,
    )> = sqlx::query_as(
        "SELECT m.id, m.subject, m.from_addr, m.to_addrs, m.deliver_at, m.size_bytes \
             FROM messages m \
             JOIN mailboxes mb ON mb.id = m.mailbox_id \
             WHERE m.tenant_id = $1 \
               AND mb.user_id = $2 \
               AND m.deliver_at IS NOT NULL \
             ORDER BY m.deliver_at ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;

    tx.commit().await?;

    let items = rows
        .into_iter()
        .map(
            |(id, subject, from_addr, to_addrs, deliver_at, size_bytes)| ScheduledItem {
                id,
                subject,
                from_addr,
                to_addrs,
                deliver_at,
                size_bytes,
            },
        )
        .collect();

    Ok(Json(items))
}

#[derive(Debug, Serialize)]
struct ScheduledCount {
    count: i64,
}

/// GET /api/v1/mail/messages/scheduled/count — retorna apenas o total de mensagens
/// agendadas do usuário, sem listá-las (sprint #423). Útil pra badges/counters de UI
/// sem o custo de serializar o body completo de list_scheduled.
async fn count_scheduled(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<ScheduledCount>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 \
           AND mb.user_id = $2 \
           AND m.deliver_at IS NOT NULL",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(ScheduledCount { count }))
}

#[derive(Debug, Serialize)]
struct SentCount {
    count: i64,
}

/// GET /api/v1/mail/messages/sent/count — total de mensagens na mailbox Sent
/// do usuário (sprint #434). Sent é identificada por `special_use = '\Sent'`
/// (RFC 6154); contamos pela join messages → mailboxes filtrando por user_id +
/// special_use. Retorna 0 quando o user nunca teve Sent criada (auto-criação
/// rola só no primeiro send via save_to_sent). Útil pra badge "X enviadas hoje"
/// num futuro filtro temporal — por ora é total all-time.
async fn count_sent(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<SentCount>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 \
           AND mb.user_id = $2 \
           AND mb.special_use = $3",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(r"\Sent")
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(SentCount { count }))
}

#[derive(Debug, Serialize)]
struct DraftsCount {
    count: i64,
}

/// GET /api/v1/mail/messages/drafts/count — total de drafts do usuário (sprint #439).
/// Drafts identificadas por `special_use = '\Drafts'` (RFC 6154); mesma técnica
/// do count_sent (#434) — JOIN messages → mailboxes filtrando por user_id +
/// special_use. Retorna 0 se o user nunca abriu compose (Drafts é auto-criada
/// no primeiro save). Útil pra badge "X rascunhos" no UI sem listar.
async fn count_drafts(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<DraftsCount>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 \
           AND mb.user_id = $2 \
           AND mb.special_use = $3",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(r"\Drafts")
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(DraftsCount { count }))
}

#[derive(Debug, Serialize)]
struct TrashCount {
    count: i64,
}

/// GET /api/v1/mail/messages/trash/count — total na lixeira do usuário (sprint #444).
/// Trash identificada por `special_use = '\Trash'` (RFC 6154); mesma técnica do
/// count_drafts (#439) e count_sent (#434). Retorna 0 se o user nunca apagou
/// nada (Trash é auto-criada no primeiro move-to-trash). Útil pra badge "X na
/// lixeira" e pra confirmar antes de empty-trash.
async fn count_trash(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<TrashCount>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 \
           AND mb.user_id = $2 \
           AND mb.special_use = $3",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(r"\Trash")
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(TrashCount { count }))
}

#[derive(Debug, Serialize)]
struct SpamCount {
    count: i64,
}

/// GET /api/v1/mail/messages/spam/count — total em spam do usuário (sprint #449).
/// Junk/spam identificada por `special_use = '\Junk'` (RFC 6154); mesma técnica do
/// count_trash (#444). Retorna 0 se o filtro nunca marcou nada (Junk é auto-criada
/// no primeiro move-to-spam). Útil pra badge "X em spam" e pra alertar usuários
/// que estão perdendo legítimos no filtro.
async fn count_spam(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<SpamCount>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.tenant_id = $1 \
           AND mb.user_id = $2 \
           AND mb.special_use = $3",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(r"\Junk")
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(SpamCount { count }))
}

#[derive(Debug, Serialize)]
struct CancelAllResult {
    cancelled: u64,
}

/// POST /api/v1/mail/messages/scheduled/cancel-all — cancela em batch todas as
/// mensagens agendadas do usuário (sprint #429), convertendo-as de volta a drafts
/// via UPDATE … SET deliver_at = NULL. Retorna `{cancelled: N}` com a contagem
/// efetivamente afetada (0 se não havia nenhuma agendada).
async fn cancel_all_scheduled(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<CancelAllResult>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let r = sqlx::query(
        "UPDATE messages SET deliver_at = NULL \
         WHERE tenant_id = $1 \
           AND deliver_at IS NOT NULL \
           AND mailbox_id IN ( \
               SELECT id FROM mailboxes WHERE tenant_id = $1 AND user_id = $2 \
           )",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    let cancelled = r.rows_affected();
    tracing::info!(
        target: "audit",
        event = "mail.cancel_all_scheduled",
        cancelled = cancelled,
        tenant_id = %ctx.tenant_id,
        user_id = %ctx.user_id,
    );

    Ok(Json(CancelAllResult { cancelled }))
}

/// POST /api/v1/mail/messages/:id/cancel-send — cancel a scheduled send (undo-send).
///
/// Clears `deliver_at` on the message, converting it back to a plain draft.
/// Returns 409 Conflict if the message was already sent (deliver_at IS NULL).
/// Returns 404 if the message doesn't belong to this user/tenant.
async fn cancel_send(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    // Verify message is owned by this user and still scheduled
    let row: Option<(bool,)> = sqlx::query_as(
        "SELECT (deliver_at IS NOT NULL) \
         FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.id = $1 AND m.tenant_id = $2 AND mb.user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;

    match row {
        None => return Err(MailError::MessageNotFound(id)),
        Some((false,)) => return Err(MailError::Conflict("message already sent".into())),
        Some((true,)) => {}
    }

    sqlx::query("UPDATE messages SET deliver_at = NULL WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    tracing::info!(target: "audit", event = "mail.cancel_send", msg_id = %id, tenant_id = %ctx.tenant_id);

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> SendRequest {
        SendRequest {
            from: "alice@acme.test".into(),
            to: vec!["bob@acme.test".into()],
            cc: None,
            bcc: None,
            subject: "hi".into(),
            body_text: Some("hello".into()),
            body_html: None,
            reply_to_id: None,
        }
    }

    #[test]
    fn ok_default() {
        assert!(validate_send_request(&req()).is_ok());
    }

    #[test]
    fn rejects_empty_recipients() {
        let mut r = req();
        r.to.clear();
        let err = format!("{:?}", validate_send_request(&r).unwrap_err());
        assert!(err.contains("no recipients"), "got: {err}");
    }

    #[test]
    fn rejects_excess_recipients() {
        let mut r = req();
        r.to = vec!["x@y.z".to_string(); 60];
        r.cc = Some(vec!["x@y.z".to_string(); 30]);
        r.bcc = Some(vec!["x@y.z".to_string(); 20]); // total 110 > 100
        let err = format!("{:?}", validate_send_request(&r).unwrap_err());
        assert!(err.contains("too many recipients"), "got: {err}");
    }

    #[test]
    fn accepts_max_recipients_exact() {
        let mut r = req();
        r.to = vec!["x@y.z".to_string(); 50];
        r.cc = Some(vec!["x@y.z".to_string(); 30]);
        r.bcc = Some(vec!["x@y.z".to_string(); 20]); // total 100
        assert!(validate_send_request(&r).is_ok());
    }

    #[test]
    fn rejects_oversize_subject() {
        let mut r = req();
        r.subject = "x".repeat(MAX_SUBJECT_BYTES + 1);
        let err = format!("{:?}", validate_send_request(&r).unwrap_err());
        assert!(err.contains("subject too large"), "got: {err}");
    }

    #[test]
    fn rejects_crlf_in_subject() {
        let mut r = req();
        r.subject = "Hi\r\nBcc: evil@x.y".into();
        let err = format!("{:?}", validate_send_request(&r).unwrap_err());
        assert!(err.contains("CR or LF"), "got: {err}");
    }

    #[test]
    fn rejects_oversize_body_combined() {
        // Sum de text+html. Cada um sozinho passaria, juntos não.
        let mut r = req();
        let half = "x".repeat(MAX_BODY_BYTES * 2 / 3);
        r.body_text = Some(half.clone());
        r.body_html = Some(half);
        let err = format!("{:?}", validate_send_request(&r).unwrap_err());
        assert!(err.contains("body too large"), "got: {err}");
    }

    /// Mirror of the clamp used in `send_with_undo` so the window bounds are
    /// unit-tested without standing up the DB/SMTP path.
    fn undo_window(requested: Option<i64>) -> i64 {
        requested
            .unwrap_or(UNDO_DEFAULT_SECONDS)
            .clamp(UNDO_MIN_SECONDS, UNDO_MAX_SECONDS)
    }

    #[test]
    fn undo_window_default_when_absent() {
        assert_eq!(undo_window(None), UNDO_DEFAULT_SECONDS);
    }

    #[test]
    fn undo_window_clamps_below_min() {
        assert_eq!(undo_window(Some(0)), UNDO_MIN_SECONDS);
        assert_eq!(undo_window(Some(-100)), UNDO_MIN_SECONDS);
    }

    #[test]
    fn undo_window_clamps_above_max() {
        assert_eq!(undo_window(Some(120)), UNDO_MAX_SECONDS);
    }

    #[test]
    fn undo_window_passes_through_in_range() {
        assert_eq!(undo_window(Some(15)), 15);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn undo_bounds_are_sane() {
        assert!(UNDO_MIN_SECONDS < UNDO_DEFAULT_SECONDS);
        assert!(UNDO_DEFAULT_SECONDS <= UNDO_MAX_SECONDS);
        assert!(UNDO_MAX_SECONDS <= 30); // Gmail's ceiling
    }

    fn itip_req() -> SendItipRequest {
        SendItipRequest {
            from: "alice@acme.test".into(),
            to: vec!["bob@acme.test".into()],
            subject: "Meeting".into(),
            method: "REQUEST".into(),
            body_text: None,
            ics: "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n".into(),
        }
    }

    #[test]
    fn itip_ok_default() {
        assert!(validate_itip_request(&itip_req()).is_ok());
    }

    #[test]
    fn itip_rejects_empty_recipients() {
        let mut r = itip_req();
        r.to.clear();
        let err = format!("{:?}", validate_itip_request(&r).unwrap_err());
        assert!(err.contains("no recipients"), "got: {err}");
    }

    #[test]
    fn itip_rejects_excess_recipients() {
        let mut r = itip_req();
        r.to = vec!["x@y.z".to_string(); MAX_RECIPIENTS_PER_MESSAGE + 1];
        let err = format!("{:?}", validate_itip_request(&r).unwrap_err());
        assert!(err.contains("too many recipients"), "got: {err}");
    }

    #[test]
    fn itip_rejects_oversize_ics() {
        let mut r = itip_req();
        r.ics = "x".repeat(MAX_ICS_BYTES + 1);
        let err = format!("{:?}", validate_itip_request(&r).unwrap_err());
        assert!(err.contains("ics payload too large"), "got: {err}");
    }

    #[test]
    fn itip_rejects_crlf_in_subject() {
        let mut r = itip_req();
        r.subject = "Meet\r\nBcc: evil@x.y".into();
        let err = format!("{:?}", validate_itip_request(&r).unwrap_err());
        assert!(err.contains("CR or LF"), "got: {err}");
    }

    #[test]
    fn itip_accepts_boundary_ics() {
        let mut r = itip_req();
        r.ics = "x".repeat(MAX_ICS_BYTES);
        assert!(validate_itip_request(&r).is_ok());
    }

    #[test]
    fn max_ics_bytes_is_256k() {
        assert_eq!(MAX_ICS_BYTES, 256 * 1024);
    }

    #[test]
    fn max_body_bytes_is_1_mib() {
        assert_eq!(MAX_BODY_BYTES, 1024 * 1024);
    }

    #[test]
    fn max_subject_bytes_is_998() {
        assert_eq!(MAX_SUBJECT_BYTES, 998);
    }

    #[test]
    fn max_recipients_per_message_is_100() {
        assert_eq!(MAX_RECIPIENTS_PER_MESSAGE, 100);
    }

    #[test]
    fn min_schedule_seconds_is_60() {
        assert_eq!(MIN_SCHEDULE_SECONDS, 60);
    }

    #[test]
    fn max_ics_bytes_is_256_kib() {
        assert_eq!(MAX_ICS_BYTES, 256 * 1024);
    }

    #[test]
    fn max_body_bytes_plus_max_ics_bytes_fits_in_usize() {
        let combined = MAX_BODY_BYTES + MAX_ICS_BYTES;
        assert!(combined > 0);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_recipients_is_positive() {
        assert!(MAX_RECIPIENTS_PER_MESSAGE > 0);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_subject_bytes_is_less_than_one_kib() {
        assert!(MAX_SUBJECT_BYTES < 1024);
    }

    #[test]
    fn max_body_bytes_is_one_mib() {
        assert_eq!(MAX_BODY_BYTES, 1024 * 1024);
    }

    #[test]
    fn max_recipients_per_message_equals_100() {
        assert_eq!(MAX_RECIPIENTS_PER_MESSAGE, 100);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn min_schedule_seconds_is_less_than_one_hour() {
        assert!(MIN_SCHEDULE_SECONDS < 3600);
    }
}

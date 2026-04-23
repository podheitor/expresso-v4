//! Scheduling endpoints — free/busy lookup + iMIP send (RFC 6638 subset).
//!
//! - GET  /api/v1/scheduling/freebusy?attendees=a@ex.com,b@ex.com&from=…&to=…
//! - POST /api/v1/scheduling/send   (body: VCALENDAR with METHOD:REQUEST, ATTENDEEs)

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::api::context::RequestCtx;
use crate::caldav::schedule;
use crate::domain::freebusy::{BusyInterval, FreeBusyRepo};
use crate::domain::event::EventRepo;
use crate::domain::{ical, itip};
use crate::error::{CalendarError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/scheduling/freebusy", get(freebusy))
        .route("/api/v1/scheduling/send",     post(send))
        .route("/api/v1/scheduling/inbox",    post(inbox))
}

/// Query shape: comma-separated `attendees`, rfc3339 `from`/`to`.
#[derive(Debug, Deserialize)]
struct FreeBusyParams {
    attendees: String,
    #[serde(with = "time::serde::rfc3339")]
    from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    to:   OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct FreeBusyResp {
    #[serde(with = "time::serde::rfc3339")]
    from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    to:   OffsetDateTime,
    attendees: std::collections::BTreeMap<String, Vec<BusyInterval>>,
}

async fn freebusy(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Query(p): Query<FreeBusyParams>,
) -> Result<impl IntoResponse> {
    if p.to <= p.from {
        return Err(CalendarError::BadRequest("`to` must be strictly after `from`".into()));
    }
    // Cap window at 370 days → bounds scan cost; covers "next year" UI needs.
    let span = p.to - p.from;
    if span > time::Duration::days(370) {
        return Err(CalendarError::BadRequest("range exceeds 370 days".into()));
    }

    let attendees: Vec<String> = p.attendees
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    if attendees.is_empty() {
        return Err(CalendarError::BadRequest("`attendees` required".into()));
    }
    if attendees.len() > 50 {
        return Err(CalendarError::BadRequest("max 50 attendees per query".into()));
    }

    let pool = state.db_or_unavailable()?;
    let map = FreeBusyRepo::new(pool)
        .lookup(ctx.tenant_id, &attendees, p.from, p.to)
        .await?;

    Ok(Json(FreeBusyResp { from: p.from, to: p.to, attendees: map }))
}

#[derive(Debug, Serialize)]
struct SendRecipient {
    email:  String,
    status: &'static str,
    message: &'static str,
}

#[derive(Debug, Serialize)]
struct SendResp {
    recipients: Vec<SendRecipient>,
}

/// POST /api/v1/scheduling/send — relay iTIP to attendees via iMIP (SMTP).
/// Body: VCALENDAR (text/calendar) with METHOD + ATTENDEE lines. Auth via
/// x-tenant-id / x-user-id (`RequestCtx`).
async fn send(
    _ctx: RequestCtx,
    body: String,
) -> std::result::Result<Json<SendResp>, StatusCode> {
    let statuses = schedule::dispatch_itip(&body).await?;
    Ok(Json(SendResp {
        recipients: statuses.into_iter().map(|(email, status, message)| {
            SendRecipient { email, status, message }
        }).collect(),
    }))
}

#[derive(Debug, Serialize)]
struct InboxResp {
    method:    Option<String>,
    uid:       Option<String>,
    attendee:  Option<String>,
    partstat:  Option<String>,
    matched:   bool,
    updated:   bool,
    /// True when REPLY rejected due to stale SEQUENCE/DTSTAMP (RFC 5546 §3.2.3).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    stale:     bool,
    /// True when a CANCEL was applied to the stored event (STATUS:CANCELLED).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    cancelled: bool,
    message:   String,
}

impl InboxResp {
    fn skeleton(method: &str, uid: String, attendee: Option<String>, partstat: Option<String>) -> Self {
        Self {
            method: Some(method.into()), uid: Some(uid),
            attendee, partstat,
            matched: false, updated: false,
            stale: false, cancelled: false,
            message: String::new(),
        }
    }
}

/// POST /api/v1/scheduling/inbox — ingest an iMIP message (RFC 5546).
///
/// Dispatches on the VCALENDAR METHOD:
/// - `REPLY`   → update PARTSTAT of the matching ATTENDEE, honouring
///               SEQUENCE/DTSTAMP staleness gate (RFC 5546 §3.2.3).
/// - `COUNTER` → logged as a pending organizer decision; stored event is
///               NOT mutated (RFC 5546 §3.2.7). Organizer must explicitly
///               accept via a fresh REQUEST or reject via DECLINECOUNTER.
/// - `REFRESH` → acknowledged; organizer-side retransmission of the latest
///               REQUEST is out of band (RFC 5546 §3.2.6). MVP: ack only.
/// - `CANCEL`  → attendee-side semantics: set `STATUS:CANCELLED` on the
///               stored event and persist (RFC 5546 §3.2.5). Idempotent.
///
/// Caller authenticates as the recipient via x-tenant-id / x-user-id.
async fn inbox(
    State(state): State<AppState>,
    ctx: RequestCtx,
    body: String,
) -> Result<Json<InboxResp>> {
    // Detect METHOD line (case-insensitive, scoped to the VCALENDAR wrapper).
    let method = body.lines()
        .map(|l| l.trim_end_matches('\r'))
        .find_map(|l| {
            let u = l.to_ascii_uppercase();
            u.strip_prefix("METHOD:").map(str::trim).map(str::to_ascii_uppercase)
        })
        .ok_or_else(|| CalendarError::BadRequest("missing METHOD".into()))?;

    let parsed = ical::parse_vevent(&body)?;
    let pool = state.db_or_unavailable()?;
    let repo = EventRepo::new(pool);

    match method.as_str() {
        "REPLY"   => handle_reply(ctx, repo, parsed, &body).await,
        "COUNTER" => handle_counter(&state, ctx, repo, parsed, &body).await,
        "REFRESH" => handle_refresh(ctx, repo, parsed).await,
        "CANCEL"  => handle_cancel(ctx, repo, parsed).await,
        other     => Err(CalendarError::BadRequest(format!(
            "unsupported METHOD: {other} (expected REPLY|COUNTER|REFRESH|CANCEL)"
        ))),
    }
}

async fn handle_reply(
    ctx:    RequestCtx,
    repo:   EventRepo<'_>,
    parsed: ical::ParsedEvent,
    body:   &str,
) -> Result<Json<InboxResp>> {
    let attendees = itip::parse_attendees(body);
    let Some(att) = attendees.into_iter().find(|a| a.partstat.is_some()) else {
        return Err(CalendarError::BadRequest("REPLY has no ATTENDEE with PARTSTAT".into()));
    };
    let partstat = att.partstat.clone().unwrap_or_else(|| "NEEDS-ACTION".into());

    let event_opt = repo.find_by_uid_in_tenant(ctx.tenant_id, &parsed.uid).await?;
    let Some(ev) = event_opt else {
        let mut r = InboxResp::skeleton("REPLY", parsed.uid, Some(att.email), Some(partstat));
        r.message = "uid not found in tenant".into();
        return Ok(Json(r));
    };

    // RFC 5546 §3.2.3: reject stale REPLYs (lower SEQUENCE, or equal SEQUENCE
    // with older DTSTAMP). Never mutate state; organizer keeps the latest view.
    let stored = ical::parse_vevent(&ev.ical_raw)?;
    if parsed.sequence < stored.sequence
        || (parsed.sequence == stored.sequence
            && matches!((parsed.dtstamp, stored.dtstamp),
                        (Some(r), Some(s)) if r < s))
    {
        let mut r = InboxResp::skeleton("REPLY", parsed.uid, Some(att.email), Some(partstat.clone()));
        r.matched = true;
        r.stale   = true;
        r.message = format!(
            "stale REPLY ignored (reply SEQUENCE={} DTSTAMP={:?} < stored SEQUENCE={} DTSTAMP={:?})",
            parsed.sequence, parsed.dtstamp, stored.sequence, stored.dtstamp
        );
        return Ok(Json(r));
    }

    let new_raw = itip::apply_rsvp(&ev.ical_raw, &att.email, &partstat)?;
    let already = new_raw == ev.ical_raw;
    if !already {
        let _ = repo.replace_by_uid(ctx.tenant_id, ev.calendar_id, &new_raw).await?;
    }
    let mut r = InboxResp::skeleton("REPLY", parsed.uid, Some(att.email), Some(partstat));
    r.matched = true;
    r.updated = !already;
    r.message = if already { "no change".into() } else { "PARTSTAT updated".into() };
    Ok(Json(r))
}

async fn handle_counter(
    state:  &AppState,
    ctx:    RequestCtx,
    repo:   EventRepo<'_>,
    parsed: ical::ParsedEvent,
    body:   &str,
) -> Result<Json<InboxResp>> {
    // RFC 5546 §3.2.7: organizer receives a proposal; MUST NOT auto-apply.
    // MVP: acknowledge + log; a future UI row lets the organizer accept
    // (by re-sending REQUEST with bumped SEQUENCE) or reject (DECLINECOUNTER).
    use crate::domain::counter::CounterRepo;
    let attendees = itip::parse_attendees(body);
    let att = attendees.into_iter().next();
    let event_opt = repo.find_by_uid_in_tenant(ctx.tenant_id, &parsed.uid).await?;
    let matched = event_opt.is_some();

    // Persist proposal so admin can accept/reject (RFC 5546 §3.2.7).
    let mut proposal_id: Option<uuid::Uuid> = None;
    if let (Some(ev), Some(ref a)) = (event_opt.as_ref(), att.as_ref()) {
        let crepo = CounterRepo::new(repo.pool());
        match crepo.insert(
            ctx.tenant_id,
            ev.id,
            &a.email,
            parsed.dtstart,
            parsed.dtend,
            None,                        // COMMENT parsing TBD
            Some(parsed.sequence),
            Some(body),
        ).await {
            Ok(p) => {
                proposal_id = Some(p.id);
                state.events().publish(crate::events::Event::CounterReceived {
                    tenant_id:      ctx.tenant_id,
                    event_id:       ev.id,
                    proposal_id:    p.id,
                    attendee_email: a.email.clone(),
                });
            }
            Err(e) => tracing::warn!(error=%e, uid=%parsed.uid, "COUNTER persist failed (non-fatal)"),
        }
    }

    tracing::info!(
        tenant_id   = %ctx.tenant_id,
        uid         = %parsed.uid,
        attendee    = att.as_ref().map(|a| a.email.as_str()).unwrap_or(""),
        sequence    = parsed.sequence,
        proposal_id = ?proposal_id,
        "iMIP COUNTER received (pending organizer decision)",
    );
    let mut r = InboxResp::skeleton(
        "COUNTER",
        parsed.uid,
        att.as_ref().map(|a| a.email.clone()),
        att.as_ref().and_then(|a| a.partstat.clone()),
    );
    r.matched = matched;
    r.message = if matched {
        format!("COUNTER received (proposal_id={}); organizer must decide (RFC 5546 §3.2.7)",
            proposal_id.map(|u| u.to_string()).unwrap_or_else(|| "none".into()))
    } else {
        "uid not found in tenant; COUNTER ignored".into()
    };
    Ok(Json(r))
}

async fn handle_refresh(
    ctx:    RequestCtx,
    repo:   EventRepo<'_>,
    parsed: ical::ParsedEvent,
) -> Result<Json<InboxResp>> {
    // RFC 5546 §3.2.6: attendee requests the latest event state. MVP: ack.
    // Organizer-initiated REQUEST resend is out of band (future: enqueue
    // outbound iMIP via schedule::dispatch_itip).
    let event_opt = repo.find_by_uid_in_tenant(ctx.tenant_id, &parsed.uid).await?;
    let matched = event_opt.is_some();
    tracing::info!(
        tenant_id = %ctx.tenant_id,
        uid = %parsed.uid,
        matched,
        "iMIP REFRESH acknowledged",
    );
    let mut r = InboxResp::skeleton("REFRESH", parsed.uid, None, None);
    r.matched = matched;
    r.message = if matched {
        "REFRESH acknowledged; organizer resend required (out of band)".into()
    } else {
        "uid not found in tenant".into()
    };
    Ok(Json(r))
}

async fn handle_cancel(
    ctx:    RequestCtx,
    repo:   EventRepo<'_>,
    parsed: ical::ParsedEvent,
) -> Result<Json<InboxResp>> {
    // RFC 5546 §3.2.5: attendee-side receipt of CANCEL → mark the event as
    // STATUS:CANCELLED in the stored ical_raw. We keep the row for audit
    // (deletion is out of scope; tombstone GC handles long-term cleanup).
    let event_opt = repo.find_by_uid_in_tenant(ctx.tenant_id, &parsed.uid).await?;
    let Some(ev) = event_opt else {
        let mut r = InboxResp::skeleton("CANCEL", parsed.uid, None, None);
        r.message = "uid not found in tenant".into();
        return Ok(Json(r));
    };

    // Staleness gate mirrors REPLY: a CANCEL with SEQUENCE < stored is rejected
    // (attendee already saw a later revision from the organizer).
    let stored = ical::parse_vevent(&ev.ical_raw)?;
    if parsed.sequence < stored.sequence {
        let mut r = InboxResp::skeleton("CANCEL", parsed.uid, None, None);
        r.matched = true;
        r.stale   = true;
        r.message = format!(
            "stale CANCEL ignored (reply SEQUENCE={} < stored SEQUENCE={})",
            parsed.sequence, stored.sequence
        );
        return Ok(Json(r));
    }

    let new_raw = itip::set_status(&ev.ical_raw, "CANCELLED")?;
    let already = new_raw == ev.ical_raw;
    if !already {
        let _ = repo.replace_by_uid(ctx.tenant_id, ev.calendar_id, &new_raw).await?;
    }
    let mut r = InboxResp::skeleton("CANCEL", parsed.uid, None, None);
    r.matched   = true;
    r.updated   = !already;
    r.cancelled = true;
    r.message   = if already { "already cancelled".into() } else { "STATUS:CANCELLED applied".into() };
    Ok(Json(r))
}

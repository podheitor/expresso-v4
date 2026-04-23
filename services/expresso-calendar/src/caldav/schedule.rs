//! CalDAV scheduling (RFC 6638) — schedule-outbox POST (iMIP sender).
//!
//! Inbox delivery currently handled via the mail service (iTIP messages
//! arrive in the user's INBOX as normal email with `text/calendar;
//! method=REQUEST` attachments). The CalDAV inbox collection is advertised
//! for client compatibility but not used for storage.
//!
//! Outbox POST body: a VCALENDAR with `METHOD:REQUEST` (or REPLY / CANCEL /
//! REFRESH). For each `ATTENDEE`, we:
//!   1. Build a MIME message (from organizer → attendee) with the iCal as
//!      both inline part and attachment (Content-Type text/calendar;
//!      method=…; charset=utf-8).
//!   2. Relay via SMTP using the configured relay (env `SMTP_HOST`,
//!      `SMTP_PORT`, optional `SMTP_USERNAME`/`SMTP_PASSWORD`).
//!
//! Response: 200 OK with a CalDAV `schedule-response` XML per RFC 6638 §6.2.
//! Each recipient gets a `<C:response>` with `<C:recipient>` and
//! `<C:request-status>` (`1.2` delivered, `3.7` invalid address,
//! `5.1` service unavailable).

use axum::{body::Body, http::StatusCode, response::Response};
use lettre::{
    message::{header::ContentType, Mailbox, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use std::str::FromStr;

use crate::caldav::auth::CalDavPrincipal;
use crate::caldav::uri::{self, Target};
use crate::caldav::xml;
use crate::domain::itip;
use crate::error::Result;
use crate::state::AppState;

const MULTISTATUS_CT: &str = r#"application/xml; charset="utf-8""#;

pub type RecipientStatus = (String, &'static str, &'static str);

/// Core iTIP dispatcher — parses body, sends per attendee, returns statuses.
/// Errors map to HTTP status codes so both CalDAV POST and JSON API can
/// consume this.
pub async fn dispatch_itip(body: &str) -> std::result::Result<Vec<RecipientStatus>, StatusCode> {
    if body.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let method = extract_method(body).unwrap_or_else(|| "REQUEST".to_string());
    let attendees = itip::parse_attendees(body);
    if attendees.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let cfg = SmtpCfg::from_env().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let transport = cfg.build_transport().map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let from_mbox: Mailbox = extract_organizer_email(body)
        .and_then(|e| e.parse().ok())
        .unwrap_or_else(|| cfg.from.clone());

    let mut responses: Vec<RecipientStatus> = Vec::with_capacity(attendees.len());
    for att in &attendees {
        let to_mbox: Mailbox = match Mailbox::from_str(&att.email) {
            Ok(m)  => m,
            Err(_) => { responses.push((att.email.clone(), "3.7", "Invalid Calendar User")); continue; }
        };
        let subject = match method.as_str() {
            "REPLY"   => "Invitation Reply",
            "CANCEL"  => "Invitation Cancelled",
            "REFRESH" => "Invitation Refresh Request",
            _         => "Meeting Invitation",
        };
        let ics_ct: ContentType = format!("text/calendar; method={method}; charset=utf-8")
            .parse()
            .expect("static content type");
        let msg_build = Message::builder()
            .from(from_mbox.clone())
            .to(to_mbox)
            .subject(subject)
            .multipart(
                MultiPart::alternative()
                    .singlepart(SinglePart::builder()
                        .header(ContentType::TEXT_PLAIN)
                        .body(format!("Calendar invitation ({method}). Your client should display the attached iCalendar object.")))
                    .singlepart(SinglePart::builder()
                        .header(ics_ct)
                        .body(body.to_owned())),
            );
        let msg = match msg_build {
            Ok(m)  => m,
            Err(_) => { responses.push((att.email.clone(), "5.1", "Message build failed")); continue; }
        };
        match transport.send(msg).await {
            Ok(_)  => responses.push((att.email.clone(), "1.2", "Message delivered")),
            Err(e) => {
                tracing::warn!(error = %e, recipient = %att.email, "iMIP SMTP send failed");
                responses.push((att.email.clone(), "5.1", "Service unavailable"));
            }
        }
    }
    Ok(responses)
}

/// POST on schedule-outbox — send iTIP to listed ATTENDEEs.
pub async fn post(
    _state:    AppState,
    principal: CalDavPrincipal,
    path:      &str,
    body:      &str,
) -> Result<Response> {
    match uri::classify(path) {
        Target::ScheduleOutbox { user_id } if user_id == principal.user_id => {}
        Target::ScheduleOutbox { .. } => return Ok(simple(StatusCode::FORBIDDEN)),
        _ => return Ok(simple(StatusCode::NOT_FOUND)),
    };

    let responses = match dispatch_itip(body).await {
        Ok(r) => r,
        Err(s) => return Ok(simple(s)),
    };

    let xml_body = render_schedule_response(&responses);
    let resp = Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", MULTISTATUS_CT)
        .body(Body::from(xml_body))
        .unwrap();
    Ok(resp)
}

/// Minimal extractor for `METHOD:<VALUE>` at VCALENDAR level.
fn extract_method(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let t = line.trim_end_matches('\r');
        if let Some(rest) = t.strip_prefix("METHOD:") {
            return Some(rest.trim().to_ascii_uppercase());
        }
    }
    None
}

/// Extract the ORGANIZER email (`mailto:` param stripped).
fn extract_organizer_email(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let t = line.trim_end_matches('\r');
        let upper = t.to_ascii_uppercase();
        if upper.starts_with("ORGANIZER") {
            let colon = t.find(':')?;
            let val = &t[colon + 1..];
            let stripped = val
                .strip_prefix("mailto:")
                .or_else(|| val.strip_prefix("MAILTO:"))
                .unwrap_or(val);
            return Some(stripped.trim().to_string());
        }
    }
    None
}

fn render_schedule_response(items: &[RecipientStatus]) -> String {
    let mut out = String::with_capacity(256 + 128 * items.len());
    out.push_str(xml::XML_PROLOG);
    out.push_str(r#"<C:schedule-response xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">"#);
    for (email, code, desc) in items {
        out.push_str("<C:response>");
        out.push_str("<C:recipient><D:href>mailto:");
        out.push_str(&xml::escape(email));
        out.push_str("</D:href></C:recipient>");
        out.push_str("<C:request-status>");
        out.push_str(code);
        out.push_str(";");
        out.push_str(desc);
        out.push_str("</C:request-status>");
        out.push_str("</C:response>");
    }
    out.push_str("</C:schedule-response>");
    out
}

// ─── SMTP config ────────────────────────────────────────────────────────────

struct SmtpCfg {
    host:     String,
    port:     u16,
    username: Option<String>,
    password: Option<String>,
    from:     Mailbox,
    starttls: bool,
}

impl SmtpCfg {
    fn from_env() -> Option<Self> {
        let host = std::env::var("SMTP_HOST").ok()?;
        let port = std::env::var("SMTP_PORT").ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(25u16);
        let username = std::env::var("SMTP_USERNAME").ok();
        let password = std::env::var("SMTP_PASSWORD").ok();
        let from_str = std::env::var("SMTP_FROM").ok()?;
        let from: Mailbox = from_str.parse().ok()?;
        let starttls = std::env::var("SMTP_STARTTLS")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        Some(Self { host, port, username, password, from, starttls })
    }

    fn build_transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
        let mut builder = if self.starttls {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.host)
                .map_err(|e| crate::error::CalendarError::BadRequest(format!("SMTP relay config: {e}")))?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.host)
        }
        .port(self.port);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            builder = builder.credentials(Credentials::new(u.clone(), p.clone()));
        }
        Ok(builder.build())
    }
}

fn simple(status: StatusCode) -> Response {
    Response::builder().status(status).body(Body::empty()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_method() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\nEND:VCALENDAR\r\n";
        assert_eq!(extract_method(ics).as_deref(), Some("REQUEST"));
    }

    #[test]
    fn missing_method_returns_none() {
        assert!(extract_method("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n").is_none());
    }


    #[test]
    fn extracts_organizer() {
        let ics = "BEGIN:VCALENDAR\r\nORGANIZER;CN=Bob:mailto:bob@ex.com\r\nEND:VCALENDAR";
        assert_eq!(extract_organizer_email(ics).as_deref(), Some("bob@ex.com"));
    }

    #[test]
    fn renders_schedule_response() {
        let items = vec![
            ("a@ex.com".to_string(), "1.2", "ok"),
            ("b@ex.com".to_string(), "3.7", "bad"),
        ];
        let out = render_schedule_response(&items);
        assert!(out.contains("<C:recipient><D:href>mailto:a@ex.com</D:href></C:recipient>"));
        assert!(out.contains("1.2;ok"));
        assert!(out.contains("3.7;bad"));
    }
}

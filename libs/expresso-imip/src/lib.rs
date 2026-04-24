//! expresso-imip — iMIP (RFC 6047) builder.
//!
//! Produces iCalendar REQUEST/CANCEL payloads + MIME multipart message
//! bodies suitable for SMTP delivery to attendees.
//!
//! Pure, no-IO. SMTP sending done by caller (typically `lettre`).
//!
//! Scope (v0.1):
//! - iCal VEVENT with METHOD:REQUEST or METHOD:CANCEL
//! - Single VEVENT per message (no recurring exceptions yet)
//! - UTC DTSTART/DTEND; no TZID/VTIMEZONE yet
//! - MIME: text/plain (human summary) + text/calendar;method=…
//!
//! Out of scope (v0.1): REPLY, COUNTER, VTIMEZONE, recurring exceptions.

use std::fmt::Write as _;
use time::format_description::well_known::Iso8601;
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum ImipError {
    #[error("format: {0}")]
    Format(#[from] std::fmt::Error),
    #[error("time format: {0}")]
    Time(#[from] time::error::Format),
    #[error("invalid input: {0}")]
    Invalid(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Request,
    Cancel,
}

impl Method {
    fn as_str(&self) -> &'static str {
        match self {
            Method::Request => "REQUEST",
            Method::Cancel => "CANCEL",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Attendee {
    pub email: String,
    pub common_name: Option<String>,
    /// RSVP=TRUE by default. False is still valid iCal.
    pub rsvp: bool,
}

#[derive(Debug, Clone)]
pub struct EventInvite {
    /// Stable UID; reuse across REQUEST/CANCEL of the same event.
    pub uid: String,
    /// Monotonic revision counter (RFC 5545 §3.8.7.4).
    pub sequence: u32,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub dtstart: OffsetDateTime,
    pub dtend: OffsetDateTime,
    pub organizer_email: String,
    pub organizer_cn: Option<String>,
    pub attendees: Vec<Attendee>,
}

/// Build an iCalendar body (VCALENDAR + VEVENT) with the given METHOD.
/// Lines are CRLF-terminated and folded at 75 octets per RFC 5545 §3.1.
pub fn build_ical(invite: &EventInvite, method: Method) -> Result<String, ImipError> {
    if invite.attendees.is_empty() {
        return Err(ImipError::Invalid("attendees empty"));
    }
    if invite.dtend <= invite.dtstart {
        return Err(ImipError::Invalid("dtend <= dtstart"));
    }

    let dtstamp = format_ical_utc(OffsetDateTime::now_utc())?;
    let dtstart = format_ical_utc(invite.dtstart)?;
    let dtend = format_ical_utc(invite.dtend)?;

    let mut s = String::new();
    // VCALENDAR
    writeln_crlf(&mut s, "BEGIN:VCALENDAR")?;
    writeln_crlf(&mut s, "VERSION:2.0")?;
    writeln_crlf(&mut s, "PRODID:-//Expresso//expresso-imip 0.1//PT")?;
    writeln_crlf(&mut s, &format!("METHOD:{}", method.as_str()))?;
    writeln_crlf(&mut s, "CALSCALE:GREGORIAN")?;
    // VEVENT
    writeln_crlf(&mut s, "BEGIN:VEVENT")?;
    writeln_crlf(&mut s, &format!("UID:{}", escape_text(&invite.uid)))?;
    writeln_crlf(&mut s, &format!("SEQUENCE:{}", invite.sequence))?;
    writeln_crlf(&mut s, &format!("DTSTAMP:{dtstamp}"))?;
    writeln_crlf(&mut s, &format!("DTSTART:{dtstart}"))?;
    writeln_crlf(&mut s, &format!("DTEND:{dtend}"))?;
    writeln_crlf(&mut s, &format!("SUMMARY:{}", escape_text(&invite.summary)))?;
    if let Some(desc) = &invite.description {
        writeln_crlf(&mut s, &format!("DESCRIPTION:{}", escape_text(desc)))?;
    }
    if let Some(loc) = &invite.location {
        writeln_crlf(&mut s, &format!("LOCATION:{}", escape_text(loc)))?;
    }
    // ORGANIZER
    let org = match &invite.organizer_cn {
        Some(cn) => format!(
            "ORGANIZER;CN={}:mailto:{}",
            quote_cn(cn),
            invite.organizer_email
        ),
        None => format!("ORGANIZER:mailto:{}", invite.organizer_email),
    };
    writeln_crlf(&mut s, &org)?;
    // ATTENDEES
    for a in &invite.attendees {
        let partstat = match method {
            Method::Request => "NEEDS-ACTION",
            Method::Cancel => "DECLINED",
        };
        let role = "REQ-PARTICIPANT";
        let rsvp = if a.rsvp { "TRUE" } else { "FALSE" };
        let line = match &a.common_name {
            Some(cn) => format!(
                "ATTENDEE;CN={};ROLE={};PARTSTAT={};RSVP={}:mailto:{}",
                quote_cn(cn),
                role,
                partstat,
                rsvp,
                a.email
            ),
            None => format!(
                "ATTENDEE;ROLE={};PARTSTAT={};RSVP={}:mailto:{}",
                role, partstat, rsvp, a.email
            ),
        };
        writeln_crlf(&mut s, &line)?;
    }
    if method == Method::Cancel {
        writeln_crlf(&mut s, "STATUS:CANCELLED")?;
    }
    writeln_crlf(&mut s, "END:VEVENT")?;
    writeln_crlf(&mut s, "END:VCALENDAR")?;
    Ok(fold_lines(&s))
}

/// Build a complete MIME multipart/mixed body for iMIP delivery.
/// Caller wraps this with SMTP headers (From/To/Subject/MIME-Version).
/// Returns (content_type, body).
pub fn build_mime_multipart(
    invite: &EventInvite,
    method: Method,
    human_text: &str,
) -> Result<(String, String), ImipError> {
    let ical = build_ical(invite, method)?;
    // Boundary: short uuid.
    let boundary = format!("=_{}=", uuid::Uuid::new_v4().simple());
    let ct = format!(
        "multipart/mixed; boundary=\"{}\"",
        boundary
    );
    let mut body = String::new();
    writeln_crlf(&mut body, &format!("--{boundary}"))?;
    writeln_crlf(&mut body, "Content-Type: text/plain; charset=UTF-8")?;
    writeln_crlf(&mut body, "Content-Transfer-Encoding: 8bit")?;
    writeln_crlf(&mut body, "")?;
    writeln_crlf(&mut body, human_text)?;
    writeln_crlf(&mut body, &format!("--{boundary}"))?;
    writeln_crlf(
        &mut body,
        &format!(
            "Content-Type: text/calendar; method={}; charset=UTF-8",
            method.as_str()
        ),
    )?;
    writeln_crlf(&mut body, "Content-Transfer-Encoding: 8bit")?;
    writeln_crlf(&mut body, "Content-Disposition: attachment; filename=\"invite.ics\"")?;
    writeln_crlf(&mut body, "")?;
    body.push_str(&ical);
    writeln_crlf(&mut body, &format!("--{boundary}--"))?;
    Ok((ct, body))
}

// --- helpers -------------------------------------------------------------

fn writeln_crlf(s: &mut String, line: &str) -> std::fmt::Result {
    write!(s, "{line}\r\n")
}

fn format_ical_utc(dt: OffsetDateTime) -> Result<String, ImipError> {
    // RFC 5545 basic UTC: YYYYMMDDTHHMMSSZ
    let iso = dt.format(&Iso8601::DEFAULT)?;
    // Strip separators + fractional: "2026-04-24T12:34:56.000000000Z"
    let mut out = String::with_capacity(16);
    for c in iso.chars() {
        if c == '-' || c == ':' {
            continue;
        }
        if c == '.' {
            break; // stop at fractional seconds
        }
        out.push(c);
    }
    // If we stopped at '.', ensure 'Z' terminator present.
    if !out.ends_with('Z') {
        out.push('Z');
    }
    Ok(out)
}

/// RFC 5545 §3.3.11 TEXT escaping: backslash, semicolon, comma, newline.
fn escape_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {} // drop CR
            _ => out.push(c),
        }
    }
    out
}

/// Quote CN parameter if it contains special chars (":", ";", ",", space).
fn quote_cn(cn: &str) -> String {
    if cn
        .chars()
        .any(|c| matches!(c, ':' | ';' | ',' | '"') || c.is_whitespace())
    {
        // RFC 5545 §3.2 param value: wrap in DQUOTE; drop internal DQUOTE.
        let cleaned: String = cn.chars().filter(|c| *c != '"').collect();
        format!("\"{cleaned}\"")
    } else {
        cn.to_string()
    }
}

/// Fold content lines longer than 75 octets per RFC 5545 §3.1.
/// Continuation lines start with a single space.
fn fold_lines(input: &str) -> String {
    const LIMIT: usize = 75;
    let mut out = String::with_capacity(input.len() + input.len() / 40);
    // Strip trailing CRLF so split doesn't yield phantom empty line.
    let trimmed = input.strip_suffix("\r\n").unwrap_or(input);
    for line in trimmed.split("\r\n") {
        let bytes = line.as_bytes();
        if bytes.is_empty() {
            out.push_str("\r\n");
            continue;
        }
        if bytes.len() <= LIMIT {
            out.push_str(line);
            out.push_str("\r\n");
            continue;
        }
        // Fold at byte boundaries that are char boundaries.
        let mut i = 0;
        let mut first = true;
        while i < line.len() {
            let chunk_len = if first { LIMIT } else { LIMIT - 1 };
            let remaining = &line[i..];
            let take = remaining
                .char_indices()
                .take_while(|(byte_idx, _)| *byte_idx < chunk_len)
                .map(|(byte_idx, c)| byte_idx + c.len_utf8())
                .last()
                .unwrap_or(remaining.len().min(chunk_len));
            let take = take.min(remaining.len());
            if !first {
                out.push(' ');
            }
            out.push_str(&remaining[..take]);
            out.push_str("\r\n");
            i += take;
            first = false;
        }
    }
    out
}

// --- tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn sample_invite() -> EventInvite {
        EventInvite {
            uid: "event-123@expresso.local".into(),
            sequence: 0,
            summary: "Daily standup".into(),
            description: Some("Sync equipe".into()),
            location: Some("Sala 1; 2º andar".into()),
            dtstart: datetime!(2026-05-10 13:00 UTC),
            dtend: datetime!(2026-05-10 13:30 UTC),
            organizer_email: "alice@expresso.local".into(),
            organizer_cn: Some("Alice Silva".into()),
            attendees: vec![
                Attendee {
                    email: "bob@expresso.local".into(),
                    common_name: Some("Bob".into()),
                    rsvp: true,
                },
                Attendee {
                    email: "carol@expresso.local".into(),
                    common_name: None,
                    rsvp: true,
                },
            ],
        }
    }

    /// Helper for tests: unfold RFC 5545 content lines.
    fn unfold(s: &str) -> String {
        s.replace("\r\n ", "")
    }

    #[test]
    fn request_contains_required_fields() {
        let ical = build_ical(&sample_invite(), Method::Request).unwrap();
        assert!(ical.starts_with("BEGIN:VCALENDAR\r\n"));
        let u = unfold(&ical);
        assert!(u.contains("METHOD:REQUEST\r\n"));
        assert!(u.contains("UID:event-123@expresso.local\r\n"));
        assert!(u.contains("SEQUENCE:0\r\n"));
        assert!(u.contains("DTSTART:20260510T130000Z\r\n"));
        assert!(u.contains("DTEND:20260510T133000Z\r\n"));
        assert!(u.contains("SUMMARY:Daily standup\r\n"));
        assert!(u.contains("ORGANIZER;CN=\"Alice Silva\":mailto:alice@expresso.local\r\n"));
        assert!(u.contains(
            "ATTENDEE;CN=Bob;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:bob@expresso.local\r\n"
        ));
        assert!(ical.contains("END:VEVENT\r\n"));
        assert!(ical.ends_with("END:VCALENDAR\r\n"));
        assert!(!ical.contains("STATUS:CANCELLED"));
    }

    #[test]
    fn cancel_sets_status_and_partstat() {
        let ical = build_ical(&sample_invite(), Method::Cancel).unwrap();
        assert!(ical.contains("METHOD:CANCEL\r\n"));
        assert!(ical.contains("STATUS:CANCELLED\r\n"));
        assert!(ical.contains("PARTSTAT=DECLINED"));
    }

    #[test]
    fn text_escaping() {
        let mut inv = sample_invite();
        inv.summary = "has; semi, comma\nnewline\\back".into();
        let ical = build_ical(&inv, Method::Request).unwrap();
        assert!(ical.contains("SUMMARY:has\\; semi\\, comma\\nnewline\\\\back\r\n"));
    }

    #[test]
    fn location_with_semicolon_escaped() {
        let ical = build_ical(&sample_invite(), Method::Request).unwrap();
        assert!(ical.contains("LOCATION:Sala 1\\; 2º andar\r\n"));
    }

    #[test]
    fn rejects_empty_attendees() {
        let mut inv = sample_invite();
        inv.attendees.clear();
        let err = build_ical(&inv, Method::Request).unwrap_err();
        matches!(err, ImipError::Invalid(_));
    }

    #[test]
    fn rejects_end_before_start() {
        let mut inv = sample_invite();
        inv.dtend = inv.dtstart;
        let err = build_ical(&inv, Method::Request).unwrap_err();
        matches!(err, ImipError::Invalid(_));
    }

    #[test]
    fn folding_respects_75_octet_limit() {
        let long = "x".repeat(200);
        let mut inv = sample_invite();
        inv.description = Some(long.clone());
        let ical = build_ical(&inv, Method::Request).unwrap();
        for line in ical.split("\r\n") {
            assert!(line.len() <= 75, "unfolded line {} bytes: {line:?}", line.len());
        }
        // Reconstruct: remove CRLF + leading space of continuation; find original substring
        let unfolded: String = ical
            .split("\r\n")
            .collect::<Vec<_>>()
            .windows(1)
            .map(|w| w[0])
            .collect::<Vec<_>>()
            .join("");
        // Weaker check: long string appears when continuation space is stripped.
        let joined = ical.replace("\r\n ", "");
        assert!(joined.contains(&long), "long desc survives unfolding");
        let _ = unfolded;
    }

    #[test]
    fn mime_multipart_has_both_parts() {
        let (ct, body) = build_mime_multipart(
            &sample_invite(),
            Method::Request,
            "Você foi convidado.",
        )
        .unwrap();
        assert!(ct.starts_with("multipart/mixed; boundary=\""));
        assert!(body.contains("Content-Type: text/plain; charset=UTF-8\r\n"));
        assert!(body.contains("Content-Type: text/calendar; method=REQUEST; charset=UTF-8\r\n"));
        assert!(body.contains("Content-Disposition: attachment; filename=\"invite.ics\"\r\n"));
        assert!(body.contains("BEGIN:VCALENDAR\r\n"));
        // Closing boundary
        assert!(body.trim_end().ends_with("--"));
    }

    #[test]
    fn format_ical_utc_strips_separators() {
        let dt = datetime!(2026-04-24 00:12:34 UTC);
        let s = format_ical_utc(dt).unwrap();
        assert_eq!(s, "20260424T001234Z");
    }
}

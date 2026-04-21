//! Minimal iCalendar (RFC 5545) property extractor.
//!
//! Line-based parse: unfold continuations, split VEVENT block, pick needed
//! properties. Full VCALENDAR kept verbatim in `ical_raw` for roundtrip
//! fidelity; this helper only exposes what the DB schema indexes.

use sha2::{Digest, Sha256};
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime};

use crate::error::{CalendarError, Result};

/// Properties extracted from a single VEVENT.
#[derive(Debug, Clone, Default)]
pub struct ParsedEvent {
    pub uid:             String,
    pub summary:         Option<String>,
    pub description:     Option<String>,
    pub location:        Option<String>,
    pub dtstart:         Option<OffsetDateTime>,
    pub dtend:           Option<OffsetDateTime>,
    pub rrule:           Option<String>,
    pub status:          Option<String>,
    pub organizer_email: Option<String>,
    pub sequence:        i32,
}

/// Parse minimal VEVENT properties from raw VCALENDAR text.
///
/// Returns the *first* VEVENT found (recurrence overrides beyond the first
/// master VEVENT are stored raw but not indexed separately).
pub fn parse_vevent(raw: &str) -> Result<ParsedEvent> {
    let lines = unfold_lines(raw);

    let mut in_event = false;
    let mut ev = ParsedEvent::default();

    for line in lines {
        let trimmed = line.trim_end_matches('\r');
        let upper = trimmed.to_ascii_uppercase();

        if upper == "BEGIN:VEVENT" {
            in_event = true;
            continue;
        }
        if upper == "END:VEVENT" {
            break; // first VEVENT only for indexing
        }
        if !in_event {
            continue;
        }

        // Split "NAME[;PARAMS]:VALUE" — only the first ':' is the separator,
        // except escaped-commas inside params which we don't support.
        let (head, value) = match trimmed.split_once(':') {
            Some(pair) => pair,
            None => continue,
        };
        let (name, params) = match head.split_once(';') {
            Some((n, p)) => (n.to_ascii_uppercase(), Some(p)),
            None         => (head.to_ascii_uppercase(), None),
        };

        match name.as_str() {
            "UID"         => ev.uid = value.to_owned(),
            "SUMMARY"     => ev.summary = Some(unescape_text(value)),
            "DESCRIPTION" => ev.description = Some(unescape_text(value)),
            "LOCATION"    => ev.location = Some(unescape_text(value)),
            "RRULE"       => ev.rrule = Some(value.to_owned()),
            "STATUS"      => ev.status = Some(value.to_ascii_uppercase()),
            "ORGANIZER"   => ev.organizer_email = extract_mailto(value),
            "SEQUENCE"    => ev.sequence = value.parse().unwrap_or(0),
            "DTSTART"     => ev.dtstart = parse_dt(params, value),
            "DTEND"       => ev.dtend   = parse_dt(params, value),
            _ => {}
        }
    }

    if ev.uid.is_empty() {
        return Err(CalendarError::InvalidICal("missing UID".into()));
    }
    Ok(ev)
}

/// Compute a stable ETag for the raw iCalendar payload (hex sha256).
pub fn compute_etag(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        use std::fmt::Write as _;
        let _ = write!(out, "{:02x}", b);
    }
    out
}

// ─── helpers ────────────────────────────────────────────────────────────────

/// Unfold RFC 5545 §3.1 line continuations (LF/CRLF followed by SP or TAB).
fn unfold_lines(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in raw.split('\n') {
        let stripped = line.strip_suffix('\r').unwrap_or(line);
        // Continuation: first char is SP or HTAB → append (minus that char) to previous.
        if let Some(first) = stripped.chars().next() {
            if (first == ' ' || first == '\t') && !out.is_empty() {
                let prev = out.last_mut().expect("non-empty");
                prev.push_str(&stripped[1..]);
                continue;
            }
        }
        out.push(stripped.to_owned());
    }
    out
}

/// RFC 5545 TEXT unescape: \n → newline, \, → ',', \; → ';', \\ → '\'.
fn unescape_text(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(',')             => out.push(','),
                Some(';')             => out.push(';'),
                Some('\\')            => out.push('\\'),
                Some(other)           => { out.push('\\'); out.push(other); }
                None                  => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Extract email address from ORGANIZER value like "mailto:user@example.org".
fn extract_mailto(v: &str) -> Option<String> {
    let v = v.trim();
    v.strip_prefix("mailto:")
        .or_else(|| v.strip_prefix("MAILTO:"))
        .map(|s| s.to_owned())
}

/// Parse DTSTART/DTEND value. Supports UTC (suffix 'Z'), naive (floating / TZID),
/// and DATE-only (VALUE=DATE) forms. Floating times are assumed UTC for storage.
fn parse_dt(params: Option<&str>, value: &str) -> Option<OffsetDateTime> {
    let is_date_only = params
        .map(|p| p.to_ascii_uppercase().contains("VALUE=DATE"))
        .unwrap_or(false)
        || value.len() == 8;

    if is_date_only && value.len() == 8 {
        let fmt = format_description!("[year][month][day]");
        return time::Date::parse(value, &fmt)
            .ok()
            .and_then(|d| d.with_hms(0, 0, 0).ok())
            .map(|p| p.assume_utc());
    }

    // Try RFC3339 first (edge case: odd clients emit it).
    if let Ok(dt) = OffsetDateTime::parse(value, &Rfc3339) {
        return Some(dt);
    }

    // UTC: "YYYYMMDDTHHMMSSZ"
    if let Some(stripped) = value.strip_suffix('Z') {
        return PrimitiveDateTime::parse(stripped, &date_time_fmt())
            .ok()
            .map(|p| p.assume_utc());
    }

    // Floating / TZID local time — treat as UTC for indexing (we preserve raw).
    PrimitiveDateTime::parse(value, &date_time_fmt())
        .ok()
        .map(|p| p.assume_utc())
}

fn date_time_fmt() -> Vec<time::format_description::FormatItem<'static>> {
    time::format_description::parse("[year][month][day]T[hour][minute][second]").unwrap()
}

/// Split a multi-event VCALENDAR into individual VEVENT blocks, each wrapped
/// in a minimal VCALENDAR container so existing single-event parsers/repo
/// methods work unchanged. Non-VEVENT components (VTIMEZONE, VTODO, …) are
/// dropped. Returns empty vec when no VEVENT blocks found.
pub fn split_vcalendar_to_events(raw: &str) -> Vec<String> {
    let lines = unfold_lines(raw);
    let mut out: Vec<String> = Vec::new();
    let mut current: Option<Vec<String>> = None;

    for line in lines {
        let trimmed = line.trim_end_matches('\r');
        let upper = trimmed.to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" {
            current = Some(vec![trimmed.to_owned()]);
            continue;
        }
        if upper == "END:VEVENT" {
            if let Some(mut buf) = current.take() {
                buf.push(trimmed.to_owned());
                let body = buf.join("\r\n");
                out.push(format!(
                    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Expresso//Import//EN\r\n{body}\r\nEND:VCALENDAR\r\n"
                ));
            }
            continue;
        }
        if let Some(buf) = current.as_mut() {
            buf.push(trimmed.to_owned());
        }
    }
    out
}

/// Extract the VEVENT block (inclusive BEGIN/END) from a VCALENDAR payload.
/// Returns None when no VEVENT is present.
pub fn extract_vevent_block(raw: &str) -> Option<String> {
    let lines = unfold_lines(raw);
    let mut buf: Vec<String> = Vec::new();
    let mut in_ev = false;
    for line in lines {
        let trimmed = line.trim_end_matches('\r');
        let upper = trimmed.to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" {
            in_ev = true;
        }
        if in_ev {
            buf.push(trimmed.to_owned());
        }
        if upper == "END:VEVENT" {
            break;
        }
    }
    if buf.is_empty() { None } else { Some(buf.join("\r\n")) }
}

/// Build a single VCALENDAR payload wrapping multiple VEVENT blocks (for export).
/// Each `vevent_block` must already be the inclusive BEGIN:VEVENT..END:VEVENT
/// form (use `extract_vevent_block` per stored event).
pub fn wrap_vcalendar(vevent_blocks: &[String]) -> String {
    let mut s = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Expresso//Export//EN\r\nCALSCALE:GREGORIAN\r\n");
    for b in vevent_blocks {
        s.push_str(b);
        if !b.ends_with("\r\n") { s.push_str("\r\n"); }
    }
    s.push_str("END:VCALENDAR\r\n");
    s
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test//EN\r\n\
BEGIN:VEVENT\r\n\
UID:abc-123@example.org\r\n\
SUMMARY:Reunião de time\r\n\
DESCRIPTION:Discutir\\nplanejamento\r\n\
LOCATION:Sala 4\r\n\
DTSTART:20260421T140000Z\r\n\
DTEND:20260421T150000Z\r\n\
RRULE:FREQ=WEEKLY;BYDAY=TU\r\n\
STATUS:CONFIRMED\r\n\
ORGANIZER:mailto:alice@example.org\r\n\
SEQUENCE:3\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn parses_basic_vevent() {
        let ev = parse_vevent(SAMPLE).unwrap();
        assert_eq!(ev.uid, "abc-123@example.org");
        assert_eq!(ev.summary.as_deref(), Some("Reunião de time"));
        assert_eq!(ev.description.as_deref(), Some("Discutir\nplanejamento"));
        assert_eq!(ev.location.as_deref(), Some("Sala 4"));
        assert_eq!(ev.rrule.as_deref(), Some("FREQ=WEEKLY;BYDAY=TU"));
        assert_eq!(ev.status.as_deref(), Some("CONFIRMED"));
        assert_eq!(ev.organizer_email.as_deref(), Some("alice@example.org"));
        assert_eq!(ev.sequence, 3);
        assert!(ev.dtstart.is_some());
        assert!(ev.dtend.is_some());
    }

    #[test]
    fn unfolds_continuations() {
        let raw = "BEGIN:VEVENT\r\nUID:u1\r\nSUMMARY:Long\r\n  description here\r\nEND:VEVENT\r\n";
        let ev = parse_vevent(raw).unwrap();
        assert_eq!(ev.summary.as_deref(), Some("Long description here"));
    }

    #[test]
    fn rejects_missing_uid() {
        let raw = "BEGIN:VEVENT\r\nSUMMARY:X\r\nEND:VEVENT\r\n";
        assert!(parse_vevent(raw).is_err());
    }

    #[test]
    fn etag_stable() {
        let e1 = compute_etag(SAMPLE);
        let e2 = compute_etag(SAMPLE);
        assert_eq!(e1, e2);
        assert_eq!(e1.len(), 64);
    }
}

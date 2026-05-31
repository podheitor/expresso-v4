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

/// A single ATTACH property (RFC 5545 §3.8.1.1).
///
/// Two forms: an external `uri` reference (`ATTACH:https://…`) or inline binary
/// (`ATTACH;ENCODING=BASE64;VALUE=BINARY:<base64>`). We index only the metadata
/// — `fmttype` (MIME), `is_inline`, and the `uri` for external refs. The raw
/// bytes of an inline attachment stay in `ical_raw`; we never copy the base64
/// blob into the index (it could be megabytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAttachment {
    /// External reference URI, or None for inline binary.
    pub uri: Option<String>,
    /// FMTTYPE parameter (MIME type), if present.
    pub fmttype: Option<String>,
    /// True when the value is inline base64 binary (VALUE=BINARY).
    pub is_inline: bool,
}

/// Properties extracted from a single VEVENT.
#[derive(Debug, Clone, Default)]
pub struct ParsedEvent {
    pub uid: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub dtstart: Option<OffsetDateTime>,
    pub dtend: Option<OffsetDateTime>,
    pub dtstamp: Option<OffsetDateTime>,
    pub rrule: Option<String>,
    pub status: Option<String>,
    pub class: Option<String>,
    pub transp: Option<String>,
    pub organizer_email: Option<String>,
    pub sequence: i32,
    pub attachments: Vec<ParsedAttachment>,
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
            None => (head.to_ascii_uppercase(), None),
        };

        match name.as_str() {
            "UID" => ev.uid = value.to_owned(),
            "SUMMARY" => ev.summary = Some(unescape_text(value)),
            "DESCRIPTION" => ev.description = Some(unescape_text(value)),
            "LOCATION" => ev.location = Some(unescape_text(value)),
            "RRULE" => ev.rrule = Some(value.to_owned()),
            "STATUS" => ev.status = Some(value.to_ascii_uppercase()),
            "CLASS" => ev.class = Some(value.to_ascii_uppercase()),
            "TRANSP" => ev.transp = Some(value.to_ascii_uppercase()),
            "ORGANIZER" => ev.organizer_email = extract_mailto(value),
            "SEQUENCE" => ev.sequence = value.parse().unwrap_or(0),
            "DTSTART" => ev.dtstart = parse_dt(params, value),
            "DTEND" => ev.dtend = parse_dt(params, value),
            "DTSTAMP" => ev.dtstamp = parse_dt(params, value),
            "ATTACH" => ev.attachments.push(parse_attach(params, value)),
            _ => {}
        }
    }

    if ev.uid.is_empty() {
        return Err(CalendarError::InvalidICal("missing UID".into()));
    }
    Ok(ev)
}

/// Properties extracted from a single VTODO (RFC 5545 §3.6.2).
#[derive(Debug, Clone, Default)]
pub struct ParsedTask {
    pub uid: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub priority: i16,
    pub percent_complete: i16,
    pub dtstart: Option<OffsetDateTime>,
    pub due: Option<OffsetDateTime>,
    pub completed: Option<OffsetDateTime>,
}

/// Parse the first VTODO's indexed properties from raw VCALENDAR text. Mirrors
/// [`parse_vevent`]; PRIORITY/PERCENT-COMPLETE default to 0 when absent or
/// unparseable. Errors only when no UID is present.
pub fn parse_vtodo(raw: &str) -> Result<ParsedTask> {
    let mut in_todo = false;
    let mut t = ParsedTask::default();

    for line in unfold_lines(raw) {
        let trimmed = line.trim_end_matches('\r');
        let upper = trimmed.to_ascii_uppercase();
        if upper == "BEGIN:VTODO" {
            in_todo = true;
            continue;
        }
        if upper == "END:VTODO" {
            break;
        }
        if !in_todo {
            continue;
        }
        let Some((head, value)) = trimmed.split_once(':') else {
            continue;
        };
        let (name, params) = match head.split_once(';') {
            Some((n, p)) => (n.to_ascii_uppercase(), Some(p)),
            None => (head.to_ascii_uppercase(), None),
        };
        match name.as_str() {
            "UID" => t.uid = value.to_owned(),
            "SUMMARY" => t.summary = Some(unescape_text(value)),
            "DESCRIPTION" => t.description = Some(unescape_text(value)),
            "STATUS" => t.status = Some(value.to_ascii_uppercase()),
            "PRIORITY" => t.priority = value.parse().unwrap_or(0).clamp(0, 9),
            "PERCENT-COMPLETE" => t.percent_complete = value.parse().unwrap_or(0).clamp(0, 100),
            "DTSTART" => t.dtstart = parse_dt(params, value),
            "DUE" => t.due = parse_dt(params, value),
            "COMPLETED" => t.completed = parse_dt(params, value),
            _ => {}
        }
    }

    if t.uid.is_empty() {
        return Err(CalendarError::InvalidICal("missing UID".into()));
    }
    Ok(t)
}

/// The fields needed to serialize a VTODO. Borrowed view over a stored task,
/// so the serializer stays decoupled from the `Task` struct.
#[derive(Debug, Clone, Copy)]
pub struct Vtodo<'a> {
    pub uid: &'a str,
    pub summary: &'a str,
    pub description: Option<&'a str>,
    pub status: &'a str,
    pub priority: i16,
    pub percent_complete: i16,
    pub dtstart: Option<OffsetDateTime>,
    pub due: Option<OffsetDateTime>,
    pub completed: Option<OffsetDateTime>,
}

/// Serialize a task's structured fields into a minimal VCALENDAR/VTODO body.
/// Used by CalDAV GET for tasks created via the REST API (no original
/// `ical_raw`). CRLF line endings per RFC 5545 §3.1.
pub fn serialize_vtodo(t: &Vtodo) -> String {
    let mut out = String::with_capacity(256);
    out.push_str("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Expresso//Calendar//EN\r\n");
    out.push_str("BEGIN:VTODO\r\n");
    push_prop(&mut out, "UID", t.uid);
    push_prop(&mut out, "SUMMARY", t.summary);
    if let Some(d) = t.description {
        push_prop(&mut out, "DESCRIPTION", d);
    }
    push_prop(&mut out, "STATUS", t.status);
    push_prop(&mut out, "PRIORITY", &t.priority.to_string());
    push_prop(
        &mut out,
        "PERCENT-COMPLETE",
        &t.percent_complete.to_string(),
    );
    if let Some(dt) = t.dtstart {
        push_prop(&mut out, "DTSTART", &format_utc(dt));
    }
    if let Some(dt) = t.due {
        push_prop(&mut out, "DUE", &format_utc(dt));
    }
    if let Some(dt) = t.completed {
        push_prop(&mut out, "COMPLETED", &format_utc(dt));
    }
    out.push_str("END:VTODO\r\nEND:VCALENDAR\r\n");
    out
}

/// Append `NAME:escaped-value` with CRLF. Text values are RFC 5545 §3.3.11
/// escaped; numeric/datetime values pass through that escape harmlessly.
fn push_prop(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push(':');
    out.push_str(&escape_text(value));
    out.push_str("\r\n");
}

/// RFC 5545 §3.3.11 TEXT escaping: backslash, newline, comma, semicolon.
fn escape_text(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            ',' => out.push_str("\\,"),
            ';' => out.push_str("\\;"),
            other => out.push(other),
        }
    }
    out
}

/// Format an instant as an RFC 5545 UTC date-time (`YYYYMMDDTHHMMSSZ`).
fn format_utc(dt: OffsetDateTime) -> String {
    let utc = dt.to_offset(time::UtcOffset::UTC);
    let fmt = format_description!("[year][month][day]T[hour][minute][second]Z");
    utc.format(&fmt).unwrap_or_default()
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
                Some(',') => out.push(','),
                Some(';') => out.push(';'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse an ATTACH property into its indexed metadata.
///
/// Inline binary (`ENCODING=BASE64` or `VALUE=BINARY`) → `is_inline = true`,
/// `uri = None` (the bytes live in ical_raw). Anything else is an external URI
/// reference and `value` is stored as `uri`. `FMTTYPE` is captured when present.
fn parse_attach(params: Option<&str>, value: &str) -> ParsedAttachment {
    let params_upper = params.map(str::to_ascii_uppercase).unwrap_or_default();
    let is_inline =
        params_upper.contains("ENCODING=BASE64") || params_upper.contains("VALUE=BINARY");
    let fmttype = params
        .and_then(|p| attach_param(p, "FMTTYPE"))
        .map(str::to_owned);
    ParsedAttachment {
        uri: if is_inline {
            None
        } else {
            Some(value.trim().to_owned())
        },
        fmttype,
        is_inline,
    }
}

/// Find a `KEY=value` parameter (case-insensitive key) in a `;`-joined param
/// string, returning its raw value. Used for FMTTYPE on ATTACH.
fn attach_param<'a>(params: &'a str, key: &str) -> Option<&'a str> {
    params.split(';').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim().eq_ignore_ascii_case(key).then(|| v.trim())
    })
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
    if buf.is_empty() {
        None
    } else {
        Some(buf.join("\r\n"))
    }
}

/// Build a single VCALENDAR payload wrapping multiple VEVENT blocks (for export).
/// Each `vevent_block` must already be the inclusive BEGIN:VEVENT..END:VEVENT
/// form (use `extract_vevent_block` per stored event).
pub fn wrap_vcalendar(vevent_blocks: &[String]) -> String {
    let mut s = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Expresso//Export//EN\r\nCALSCALE:GREGORIAN\r\n");
    for b in vevent_blocks {
        s.push_str(b);
        if !b.ends_with("\r\n") {
            s.push_str("\r\n");
        }
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

    #[test]
    fn parses_dtstamp() {
        let raw = "BEGIN:VEVENT\r\nUID:u1\r\nDTSTAMP:20260423T120000Z\r\nEND:VEVENT\r\n";
        let ev = parse_vevent(raw).unwrap();
        assert!(ev.dtstamp.is_some());
        let s = ev.dtstamp.unwrap();
        assert_eq!(s.unix_timestamp(), 1776945600);
    }

    #[test]
    fn missing_dtstamp_is_none() {
        let raw = "BEGIN:VEVENT\r\nUID:u1\r\nSUMMARY:x\r\nEND:VEVENT\r\n";
        let ev = parse_vevent(raw).unwrap();
        assert!(ev.dtstamp.is_none());
    }

    #[test]
    fn unknown_property_is_ignored() {
        let raw = "BEGIN:VEVENT\r\nUID:u1\r\nX-CUSTOM-PROP:arbitrary value\r\nEND:VEVENT\r\n";
        let ev = parse_vevent(raw).unwrap();
        assert_eq!(ev.uid, "u1");
    }

    #[test]
    fn empty_input_returns_error() {
        assert!(parse_vevent("").is_err());
    }

    #[test]
    fn missing_dtstart_is_accepted() {
        // DTSTART is optional in our indexing model (stored raw; only indexed when present).
        let raw = "BEGIN:VEVENT\r\nUID:u2\r\nSUMMARY:No start\r\nEND:VEVENT\r\n";
        let ev = parse_vevent(raw).unwrap();
        assert!(ev.dtstart.is_none());
    }

    #[test]
    fn very_long_summary_does_not_panic() {
        // Simulate an allocation-bomb attempt: a SUMMARY value of 256 KiB.
        let big_value = "A".repeat(256 * 1024);
        let raw = format!("BEGIN:VEVENT\r\nUID:u3\r\nSUMMARY:{big_value}\r\nEND:VEVENT\r\n");
        let ev = parse_vevent(&raw).unwrap();
        assert_eq!(ev.summary.as_deref().map(|s| s.len()), Some(256 * 1024));
    }

    #[test]
    fn sequence_defaults_to_zero_on_invalid() {
        let raw = "BEGIN:VEVENT\r\nUID:u4\r\nSEQUENCE:not-a-number\r\nEND:VEVENT\r\n";
        let ev = parse_vevent(raw).unwrap();
        assert_eq!(ev.sequence, 0);
    }

    #[test]
    fn etag_differs_for_different_inputs() {
        assert_ne!(compute_etag("one"), compute_etag("two"));
    }

    #[test]
    fn etag_same_input_same_output() {
        assert_eq!(compute_etag("hello"), compute_etag("hello"));
    }

    #[test]
    fn etag_nonempty_for_nonempty_input() {
        assert!(!compute_etag("BEGIN:VCALENDAR").is_empty());
    }

    #[test]
    fn etag_is_hex_string() {
        let e = compute_etag("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n");
        assert!(e.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn etag_length_is_fixed() {
        // SHA-256 hex = 64 characters
        let e = compute_etag("some content");
        assert_eq!(e.len(), 64);
    }

    #[test]
    fn parse_vevent_uid_with_special_chars() {
        let raw = "BEGIN:VEVENT\r\nUID:special+uid_123@host.example\r\nEND:VEVENT\r\n";
        let ev = parse_vevent(raw).unwrap();
        assert_eq!(ev.uid, "special+uid_123@host.example");
    }

    #[test]
    fn unescape_backslash_n_decoded_to_newline() {
        let raw = "BEGIN:VEVENT\r\nUID:u1\r\nSUMMARY:line1\\nline2\r\nEND:VEVENT\r\n";
        let ev = parse_vevent(raw).unwrap();
        assert_eq!(ev.summary.as_deref(), Some("line1\nline2"));
    }

    #[test]
    fn etag_two_different_inputs_differ() {
        let a = compute_etag("input-one");
        let b = compute_etag("input-two");
        assert_ne!(a, b);
    }

    #[test]
    fn etag_empty_input_has_fixed_length() {
        let e = compute_etag("");
        assert_eq!(e.len(), compute_etag("x").len());
    }

    #[test]
    fn parse_vevent_summary_extracted() {
        let raw = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:s@x\r\nSUMMARY:Hello World\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let ev = parse_vevent(raw).unwrap();
        assert_eq!(ev.summary.as_deref(), Some("Hello World"));
    }

    #[test]
    fn compute_etag_short_input_has_fixed_length() {
        assert_eq!(compute_etag("x").len(), compute_etag("hello world").len());
    }

    #[test]
    fn parse_vevent_sequence_zero_on_missing() {
        let raw = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:seq@x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let ev = parse_vevent(raw).unwrap();
        assert_eq!(ev.sequence, 0);
    }

    // ---- ATTACH ----

    #[test]
    fn parses_uri_attachment() {
        let raw = "BEGIN:VEVENT\r\nUID:a1\r\nATTACH:https://files.example.org/agenda.pdf\r\nEND:VEVENT\r\n";
        let ev = parse_vevent(raw).unwrap();
        assert_eq!(ev.attachments.len(), 1);
        let a = &ev.attachments[0];
        assert_eq!(
            a.uri.as_deref(),
            Some("https://files.example.org/agenda.pdf")
        );
        assert!(!a.is_inline);
        assert!(a.fmttype.is_none());
    }

    #[test]
    fn parses_uri_attachment_with_fmttype() {
        let raw = "BEGIN:VEVENT\r\nUID:a2\r\nATTACH;FMTTYPE=application/pdf:https://x.test/d.pdf\r\nEND:VEVENT\r\n";
        let ev = parse_vevent(raw).unwrap();
        let a = &ev.attachments[0];
        assert_eq!(a.fmttype.as_deref(), Some("application/pdf"));
        assert_eq!(a.uri.as_deref(), Some("https://x.test/d.pdf"));
    }

    #[test]
    fn parses_inline_binary_attachment_without_storing_blob() {
        let raw = "BEGIN:VEVENT\r\nUID:a3\r\nATTACH;FMTTYPE=text/plain;ENCODING=BASE64;VALUE=BINARY:SGVsbG8=\r\nEND:VEVENT\r\n";
        let ev = parse_vevent(raw).unwrap();
        let a = &ev.attachments[0];
        assert!(a.is_inline);
        assert!(a.uri.is_none()); // blob stays in ical_raw, not indexed
        assert_eq!(a.fmttype.as_deref(), Some("text/plain"));
    }

    #[test]
    fn collects_multiple_attachments() {
        let raw = "BEGIN:VEVENT\r\nUID:a4\r\nATTACH:https://a.test/1\r\nATTACH:https://a.test/2\r\nEND:VEVENT\r\n";
        let ev = parse_vevent(raw).unwrap();
        assert_eq!(ev.attachments.len(), 2);
    }

    #[test]
    fn no_attachment_yields_empty_vec() {
        let ev = parse_vevent(SAMPLE).unwrap();
        assert!(ev.attachments.is_empty());
    }

    // ---- VTODO ----

    #[test]
    fn parse_vtodo_extracts_fields() {
        let raw = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:t1@x\r\nSUMMARY:Buy milk\r\n\
                   STATUS:IN-PROCESS\r\nPRIORITY:3\r\nPERCENT-COMPLETE:40\r\n\
                   DUE:20260601T120000Z\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let t = parse_vtodo(raw).unwrap();
        assert_eq!(t.uid, "t1@x");
        assert_eq!(t.summary.as_deref(), Some("Buy milk"));
        assert_eq!(t.status.as_deref(), Some("IN-PROCESS"));
        assert_eq!(t.priority, 3);
        assert_eq!(t.percent_complete, 40);
        assert!(t.due.is_some());
    }

    #[test]
    fn parse_vtodo_requires_uid() {
        let raw =
            "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nSUMMARY:no uid\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        assert!(parse_vtodo(raw).is_err());
    }

    #[test]
    fn parse_vtodo_clamps_out_of_range_priority_and_percent() {
        let raw = "BEGIN:VTODO\r\nUID:t2\r\nPRIORITY:99\r\nPERCENT-COMPLETE:250\r\nEND:VTODO\r\n";
        let t = parse_vtodo(raw).unwrap();
        assert_eq!(t.priority, 9);
        assert_eq!(t.percent_complete, 100);
    }

    #[test]
    fn serialize_vtodo_roundtrips_through_parse() {
        let ics = serialize_vtodo(&Vtodo {
            uid: "rt@x",
            summary: "Roundtrip; with, specials",
            description: Some("line1\nline2"),
            status: "NEEDS-ACTION",
            priority: 5,
            percent_complete: 0,
            dtstart: None,
            due: None,
            completed: None,
        });
        assert!(ics.contains("BEGIN:VTODO"));
        let t = parse_vtodo(&ics).unwrap();
        assert_eq!(t.uid, "rt@x");
        assert_eq!(t.summary.as_deref(), Some("Roundtrip; with, specials"));
        assert_eq!(t.description.as_deref(), Some("line1\nline2"));
        assert_eq!(t.priority, 5);
    }

    #[test]
    fn serialize_vtodo_emits_utc_due() {
        let due = time::macros::datetime!(2026-06-01 12:00:00 UTC);
        let ics = serialize_vtodo(&Vtodo {
            uid: "d@x",
            summary: "due",
            description: None,
            status: "NEEDS-ACTION",
            priority: 0,
            percent_complete: 0,
            dtstart: None,
            due: Some(due),
            completed: None,
        });
        assert!(ics.contains("DUE:20260601T120000Z"), "got: {ics}");
    }
}

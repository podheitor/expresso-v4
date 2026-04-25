//! iTIP (RFC 5546) — REQUEST/REPLY iCal builders + attendee helpers.
//!
//! Kept minimal: the VEVENT already lives in `calendar_events.ical_raw`;
//! iTIP adds a METHOD property at VCALENDAR scope + lets us mutate a
//! single ATTENDEE's PARTSTAT for RSVP.

use crate::domain::ical::{self, ParsedEvent};
use crate::error::{CalendarError, Result};

/// Per-attendee snapshot parsed from a VEVENT block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attendee {
    pub email:    String,
    pub cn:       Option<String>,
    pub role:     Option<String>,   // e.g. REQ-PARTICIPANT
    pub partstat: Option<String>,   // NEEDS-ACTION | ACCEPTED | DECLINED | TENTATIVE
    pub rsvp:     Option<bool>,
}

/// Extract ATTENDEEs from the first VEVENT of a VCALENDAR.
pub fn parse_attendees(raw: &str) -> Vec<Attendee> {
    let lines = unfold(raw);
    let mut in_event = false;
    let mut out = Vec::new();
    for line in lines {
        let trimmed = line.trim_end_matches('\r');
        let upper = trimmed.to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" { in_event = true; continue; }
        if upper == "END:VEVENT"   { break; }
        if !in_event { continue; }

        let (head, value) = match trimmed.split_once(':') {
            Some(p) => p,
            None => continue,
        };
        let (name, params) = match head.split_once(';') {
            Some((n, p)) => (n.to_ascii_uppercase(), Some(p)),
            None         => (head.to_ascii_uppercase(), None),
        };
        if name != "ATTENDEE" { continue; }

        let email = match extract_mailto(value) {
            Some(e) => e,
            None    => continue,
        };
        let mut a = Attendee { email, cn: None, role: None, partstat: None, rsvp: None };
        if let Some(p) = params {
            for part in p.split(';') {
                let (k, v) = match part.split_once('=') {
                    Some(kv) => kv,
                    None => continue,
                };
                match k.to_ascii_uppercase().as_str() {
                    "CN"       => a.cn = Some(v.trim_matches('"').to_owned()),
                    "ROLE"     => a.role = Some(v.to_ascii_uppercase()),
                    "PARTSTAT" => a.partstat = Some(v.to_ascii_uppercase()),
                    "RSVP"     => a.rsvp = Some(v.eq_ignore_ascii_case("TRUE")),
                    _ => {}
                }
            }
        }
        out.push(a);
    }
    out
}

/// Extract the COMMENT property from the first VEVENT, if any. Used by the
/// COUNTER inbox path so the proposal stored in `event_counter_proposals`
/// carries the rationale the attendee wrote ("Could we move this 30min?").
/// Returns `None` for empty or absent COMMENT. Honours iCal line unfolding
/// (RFC 5545 §3.1) — multi-line COMMENTs are joined back into one string.
/// Escape sequences (`\n`, `\,`, `\;`, `\\`) are decoded per RFC 5545 §3.3.11.
pub fn parse_comment(raw: &str) -> Option<String> {
    let lines = unfold(raw);
    let mut in_event = false;
    for line in lines {
        let trimmed = line.trim_end_matches('\r');
        let upper = trimmed.to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" { in_event = true; continue; }
        if upper == "END:VEVENT"   { break; }
        if !in_event { continue; }

        // COMMENT may carry params (LANGUAGE=en-US:hello). Match the property
        // name up to ';' or ':' and ignore params — we only want the value.
        let (head, value) = trimmed.split_once(':')?;
        let name = head.split(';').next()?.to_ascii_uppercase();
        if name != "COMMENT" { continue; }
        let decoded = decode_text(value);
        if decoded.is_empty() { return None; }
        return Some(decoded);
    }
    None
}

fn decode_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' { out.push(c); continue; }
        match chars.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some(',')             => out.push(','),
            Some(';')             => out.push(';'),
            Some('\\')            => out.push('\\'),
            Some(other)           => { out.push('\\'); out.push(other); }
            None                  => out.push('\\'),
        }
    }
    out
}

/// Wrap a stored VEVENT (raw VCALENDAR) with `METHOD:REQUEST` so SMTP/Mail
/// clients recognise it as a scheduling invitation. Replaces any existing
/// METHOD line. Returns the full VCALENDAR text.
pub fn build_request(raw: &str) -> Result<String> {
    // Validate by parsing UID — guarantees raw is a sensible VCALENDAR.
    let _p: ParsedEvent = ical::parse_vevent(raw)?;
    Ok(inject_method(raw, "REQUEST"))
}

/// Build a METHOD:REPLY iCal carrying exactly ONE ATTENDEE (the responding
/// user) with the given PARTSTAT. Organizer + UID + DTSTAMP/DTSTART/DTEND
/// are preserved from the master event; SEQUENCE is bumped only if caller
/// chooses to in a later flow (not required for REPLY).
pub fn build_reply(raw: &str, email: &str, partstat: &str) -> Result<String> {
    let parsed = ical::parse_vevent(raw)?;
    let organizer = parsed.organizer_email
        .as_deref()
        .ok_or_else(|| CalendarError::BadRequest("event has no ORGANIZER — cannot REPLY".into()))?;

    let mut s = String::new();
    s.push_str("BEGIN:VCALENDAR\r\n");
    s.push_str("VERSION:2.0\r\n");
    s.push_str("PRODID:-//Expresso//iTIP//EN\r\n");
    s.push_str("METHOD:REPLY\r\n");
    s.push_str("BEGIN:VEVENT\r\n");
    s.push_str(&format!("UID:{}\r\n", parsed.uid));
    if let Some(dt) = parsed.dtstart { let _ = write_dt(&mut s, "DTSTART", dt); }
    if let Some(dt) = parsed.dtend   { let _ = write_dt(&mut s, "DTEND",   dt); }
    if let Some(summary) = &parsed.summary {
        s.push_str(&format!("SUMMARY:{}\r\n", summary.replace('\r', " ").replace('\n', "\\n")));
    }
    s.push_str(&format!("ORGANIZER:mailto:{organizer}\r\n"));
    s.push_str(&format!("ATTENDEE;PARTSTAT={}:mailto:{}\r\n", partstat.to_ascii_uppercase(), email));
    s.push_str("END:VEVENT\r\n");
    s.push_str("END:VCALENDAR\r\n");
    Ok(s)
}

/// Update the stored VCALENDAR raw text: set the PARTSTAT parameter on the
/// ATTENDEE line whose mailto matches `email` (case-insensitive). If no
/// matching ATTENDEE exists, one is appended. Returns the new raw text.
pub fn apply_rsvp(raw: &str, email: &str, partstat: &str) -> Result<String> {
    validate_partstat(partstat)?;
    // Operate on physical (folded) lines so we preserve original formatting.
    let mut lines: Vec<String> = raw.split('\n').map(|s| s.trim_end_matches('\r').to_owned()).collect();
    let mut updated = false;
    let target = email.to_ascii_lowercase();

    for line in lines.iter_mut() {
        if !line.to_ascii_uppercase().starts_with("ATTENDEE") { continue; }
        let (head, value) = match line.split_once(':') {
            Some(p) => (p.0.to_owned(), p.1.to_owned()),
            None => continue,
        };
        let line_email = match extract_mailto(&value) {
            Some(e) => e,
            None => continue,
        };
        if line_email.to_ascii_lowercase() != target { continue; }

        // Replace PARTSTAT=… param (or add one) in `head`.
        let mut segs: Vec<String> = head.split(';').map(|s| s.to_owned()).collect();
        let mut found_partstat = false;
        for seg in segs.iter_mut().skip(1) {
            let upper = seg.to_ascii_uppercase();
            if upper.starts_with("PARTSTAT=") {
                *seg = format!("PARTSTAT={}", partstat.to_ascii_uppercase());
                found_partstat = true;
            }
        }
        if !found_partstat {
            segs.push(format!("PARTSTAT={}", partstat.to_ascii_uppercase()));
        }
        *line = format!("{}:{}", segs.join(";"), value);
        updated = true;
        break;
    }

    if !updated {
        // Append new ATTENDEE before END:VEVENT.
        let new_line = format!(
            "ATTENDEE;PARTSTAT={}:mailto:{}",
            partstat.to_ascii_uppercase(),
            email,
        );
        let pos = lines.iter().position(|l| l.eq_ignore_ascii_case("END:VEVENT"))
            .ok_or_else(|| CalendarError::InvalidICal("END:VEVENT not found".into()))?;
        lines.insert(pos, new_line);
    }

    Ok(lines.join("\r\n"))
}

/// Force `STATUS:<value>` on the first VEVENT of `raw`. Replaces existing
/// STATUS line when present, otherwise inserts one just before `END:VEVENT`.
/// Returns the new raw text. Used to apply METHOD:CANCEL at the attendee side
/// (`STATUS:CANCELLED` per RFC 5546 §3.2.5).
pub fn set_status(raw: &str, status: &str) -> Result<String> {
    let status = status.to_ascii_uppercase();
    let mut lines: Vec<String> = raw.split('\n').map(|s| s.trim_end_matches('\r').to_owned()).collect();
    let mut in_event = false;
    let mut replaced = false;
    for line in lines.iter_mut() {
        let upper = line.to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" { in_event = true; continue; }
        if upper == "END:VEVENT"   { break; }
        if !in_event { continue; }
        if upper.starts_with("STATUS:") || upper.starts_with("STATUS;") {
            *line = format!("STATUS:{status}");
            replaced = true;
        }
    }
    if !replaced {
        let pos = lines.iter().position(|l| l.eq_ignore_ascii_case("END:VEVENT"))
            .ok_or_else(|| CalendarError::InvalidICal("END:VEVENT not found".into()))?;
        lines.insert(pos, format!("STATUS:{status}"));
    }
    Ok(lines.join("\r\n"))
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn inject_method(raw: &str, method: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 32);
    let mut injected = false;
    for line in raw.split('\n') {
        let trimmed = line.trim_end_matches('\r');
        let upper = trimmed.to_ascii_uppercase();
        if upper.starts_with("METHOD:") {
            // skip → will inject ours
            continue;
        }
        out.push_str(trimmed);
        out.push_str("\r\n");
        if !injected && upper.starts_with("BEGIN:VCALENDAR") {
            out.push_str(&format!("METHOD:{method}\r\n"));
            injected = true;
        }
    }
    out
}

fn extract_mailto(v: &str) -> Option<String> {
    let lower = v.to_ascii_lowercase();
    let rest = lower.strip_prefix("mailto:").unwrap_or(&lower);
    let cleaned = rest.trim();
    if cleaned.contains('@') { Some(cleaned.to_owned()) } else { None }
}

fn validate_partstat(p: &str) -> Result<()> {
    match p.to_ascii_uppercase().as_str() {
        "NEEDS-ACTION" | "ACCEPTED" | "DECLINED" | "TENTATIVE" | "DELEGATED" => Ok(()),
        _ => Err(CalendarError::BadRequest(format!("invalid PARTSTAT: {p}"))),
    }
}

fn unfold(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in raw.split('\n') {
        let s = line.strip_suffix('\r').unwrap_or(line);
        if let Some(first) = s.chars().next() {
            if (first == ' ' || first == '\t') && !out.is_empty() {
                out.last_mut().unwrap().push_str(&s[1..]);
                continue;
            }
        }
        out.push(s.to_owned());
    }
    out
}

fn write_dt(s: &mut String, name: &str, dt: time::OffsetDateTime) -> std::fmt::Result {
    use std::fmt::Write;
    let fmt = time::format_description::parse("[year][month][day]T[hour][minute][second]Z").unwrap();
    let out = dt.to_offset(time::UtcOffset::UTC).format(&fmt).unwrap_or_default();
    writeln!(s, "{}:{}\r", name, out)
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test//EN\r\n\
BEGIN:VEVENT\r\n\
UID:evt-1@expresso\r\n\
DTSTART:20260501T140000Z\r\n\
DTEND:20260501T150000Z\r\n\
SUMMARY:Invite test\r\n\
ORGANIZER:mailto:alice@example.org\r\n\
ATTENDEE;CN=\"Bob\";PARTSTAT=NEEDS-ACTION;ROLE=REQ-PARTICIPANT;RSVP=TRUE:mailto:bob@example.org\r\n\
ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:carol@example.org\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn parses_two_attendees() {
        let a = parse_attendees(SAMPLE);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].email, "bob@example.org");
        assert_eq!(a[0].cn.as_deref(), Some("Bob"));
        assert_eq!(a[0].partstat.as_deref(), Some("NEEDS-ACTION"));
        assert_eq!(a[0].role.as_deref(), Some("REQ-PARTICIPANT"));
        assert_eq!(a[0].rsvp, Some(true));
        assert_eq!(a[1].email, "carol@example.org");
    }

    #[test]
    fn build_request_injects_method() {
        let out = build_request(SAMPLE).unwrap();
        assert!(out.contains("METHOD:REQUEST"));
        assert!(out.contains("UID:evt-1@expresso"));
        // Only one METHOD line in output.
        assert_eq!(out.matches("METHOD:").count(), 1);
    }

    #[test]
    fn build_reply_has_single_attendee() {
        let r = build_reply(SAMPLE, "bob@example.org", "ACCEPTED").unwrap();
        assert!(r.contains("METHOD:REPLY"));
        assert!(r.contains("ATTENDEE;PARTSTAT=ACCEPTED:mailto:bob@example.org"));
        assert!(r.contains("ORGANIZER:mailto:alice@example.org"));
        assert_eq!(r.matches("ATTENDEE").count(), 1);
    }

    #[test]
    fn apply_rsvp_updates_partstat() {
        let updated = apply_rsvp(SAMPLE, "bob@example.org", "ACCEPTED").unwrap();
        assert!(updated.contains("PARTSTAT=ACCEPTED"));
        // carol still NEEDS-ACTION
        assert!(updated.contains("mailto:carol@example.org"));
        let atts = parse_attendees(&updated);
        let bob = atts.iter().find(|a| a.email == "bob@example.org").unwrap();
        assert_eq!(bob.partstat.as_deref(), Some("ACCEPTED"));
    }

    #[test]
    fn apply_rsvp_appends_when_missing() {
        let updated = apply_rsvp(SAMPLE, "new@example.org", "TENTATIVE").unwrap();
        let atts = parse_attendees(&updated);
        assert!(atts.iter().any(|a| a.email == "new@example.org" && a.partstat.as_deref() == Some("TENTATIVE")));
    }

    #[test]
    fn apply_rsvp_rejects_bad_partstat() {
        assert!(apply_rsvp(SAMPLE, "bob@example.org", "WAT").is_err());
    }

    #[test]
    fn set_status_replaces_existing() {
        let raw = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:u1\r\nSTATUS:CONFIRMED\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let out = set_status(raw, "CANCELLED").unwrap();
        assert!(out.contains("STATUS:CANCELLED"));
        assert!(!out.contains("STATUS:CONFIRMED"));
        assert_eq!(out.matches("STATUS:").count(), 1);
    }

    #[test]
    fn set_status_inserts_when_absent() {
        let raw = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:u1\r\nSUMMARY:x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let out = set_status(raw, "CANCELLED").unwrap();
        assert!(out.contains("STATUS:CANCELLED"));
    }

    #[test]
    fn parse_comment_decodes_escapes_and_unfolds() {
        let raw = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:u1\r\n\
                   COMMENT:Could we shift 30min?\\n-- thx\\, Bob\r\n\
                   END:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(parse_comment(raw).as_deref(), Some("Could we shift 30min?\n-- thx, Bob"));
    }

    #[test]
    fn parse_comment_skips_params_and_unfolds_continuation() {
        // RFC 5545 §3.1 line folding: leading SP/TAB joins continuation onto
        // the previous logical line. parse_comment must see the joined value.
        let raw = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:u1\r\n\
                   COMMENT;LANGUAGE=en-US:hello\r\n there\r\n\
                   END:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(parse_comment(raw).as_deref(), Some("hellothere"));
    }

    #[test]
    fn parse_comment_returns_none_when_absent_or_empty() {
        assert_eq!(parse_comment(SAMPLE), None);
        let empty = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:u1\r\nCOMMENT:\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(parse_comment(empty), None);
    }
}

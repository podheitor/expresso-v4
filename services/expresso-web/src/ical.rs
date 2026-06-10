//! iCalendar (RFC 5545) emit/parse helpers for the calendar event forms.
//!
//! These are pure string transforms: the web tier builds a VCALENDAR from an
//! [`EventForm`] and ships it to the calendar backend, which parses + indexes
//! it on ingest. The reverse helpers ([`valarm_minutes`], [`categories_from_ical`])
//! seed the edit form back from a stored event's raw iCalendar.

use serde::Deserialize;
use time::{macros::format_description, OffsetDateTime};

#[derive(Deserialize)]
pub struct EventForm {
    pub summary: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub description: String,
    pub dtstart: String, // "YYYY-MM-DDTHH:MM"
    pub dtend: String,
    #[serde(default)]
    pub attendees: String, // newline / comma / semicolon separated
    /// Reminder lead times in minutes before start, comma-separated (e.g.
    /// "15,60"). Each becomes a VALARM with a relative `-PT{m}M` trigger.
    #[serde(default)]
    pub reminders: String,
    /// Comma-separated event categories (RFC 5545 CATEGORIES). The backend
    /// parses + indexes these from the iCalendar on save.
    #[serde(default)]
    pub categories: String,
    /// Comma-separated emails of bookable resources (rooms/equipment) to reserve.
    /// Each is emitted as an `ATTENDEE;CUTYPE=ROOM`; the backend records the
    /// booking and detects double-bookings.
    #[serde(default)]
    pub resources: String,
    /// Newline/comma-separated attachment URLs (RFC 5545 ATTACH). Each becomes
    /// an `ATTACH:<uri>` line; the backend indexes them. Files aren't uploaded —
    /// only links (e.g. a drive file URL or an external doc).
    #[serde(default)]
    pub attachments: String,
}

/// Split attachment URLs (newline/comma/whitespace-separated), keeping only
/// http(s) and known URI schemes, deduped, capped to bound output.
pub fn parse_attachment_uris(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = raw
        .split(['\n', ',', ' ', '\t'])
        .map(str::trim)
        .filter(|s| {
            s.starts_with("http://")
                || s.starts_with("https://")
                || s.starts_with("mailto:")
                || s.starts_with("cid:")
        })
        .map(str::to_string)
        .collect();
    out.dedup();
    out.truncate(20);
    out
}

/// Split a comma/semicolon/whitespace-separated list of resource emails into a
/// deduped, lowercased list (mirrors attendee parsing, capped to bound output).
pub fn parse_resource_emails(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = raw
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.contains('@'))
        .map(str::to_ascii_lowercase)
        .collect();
    out.sort_unstable();
    out.dedup();
    out.truncate(50);
    out
}

/// Normalize a browser `datetime-local` value ("YYYY-MM-DDTHH:MM") to RFC 3339:
/// append `:00` seconds when absent and a trailing `Z`. A value that already
/// carries a `Z` or a `+`/`-` offset in its time portion is passed through.
/// Returns None for blank input.
pub fn to_rfc3339(local: &str) -> Option<String> {
    let s = local.trim();
    if s.is_empty() {
        return None;
    }
    // Has an offset or Z already → assume caller sent RFC3339.
    if s.ends_with('Z') || s[11.min(s.len())..].contains(['+', '-']) {
        return Some(s.to_string());
    }
    let with_secs = if s.len() == 16 {
        format!("{s}:00")
    } else {
        s.to_string()
    };
    Some(format!("{with_secs}Z"))
}

/// Convert "YYYY-MM-DDTHH:MM" → iCal "YYYYMMDDTHHMMSSZ" (assume UTC input for MVP).
fn local_to_ical_utc(s: &str) -> Option<String> {
    // accept "YYYY-MM-DDTHH:MM" or "YYYY-MM-DDTHH:MM:SS"
    let (date, rest) = s.split_once('T')?;
    let (h, m) = rest.get(0..2).zip(rest.get(3..5))?;
    let date_compact: String = date.chars().filter(|c| *c != '-').collect();
    Some(format!("{date_compact}T{h}{m}00Z"))
}

pub fn escape_ical(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

/// Parse a comma-separated minutes-before list ("15,60") into VALARM blocks
/// with a DISPLAY action and a `-PT{m}M` relative trigger. Skips blanks,
/// non-numbers, and negatives; dedups; caps at 10 to bound the payload.
pub fn build_valarms(reminders: &str, summary: &str) -> String {
    let mut mins: Vec<u32> = reminders
        .split(',')
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .collect();
    mins.sort_unstable();
    mins.dedup();
    mins.truncate(10);
    let mut out = String::new();
    for m in mins {
        out.push_str("BEGIN:VALARM\r\n");
        out.push_str("ACTION:DISPLAY\r\n");
        out.push_str(&format!("TRIGGER:-PT{m}M\r\n"));
        out.push_str(&format!("DESCRIPTION:{}\r\n", escape_ical(summary)));
        out.push_str("END:VALARM\r\n");
    }
    out
}

/// Extract reminder lead times (minutes) from an event's iCalendar by reading
/// each VALARM's `TRIGGER:-PT{n}M` / `-PT{n}H` / `-P{n}D`. Returns a sorted,
/// deduped, comma-separated minutes list to seed the edit form. Best-effort:
/// triggers it can't parse as a simple negative duration are skipped.
pub fn valarm_minutes(ical: &str) -> String {
    let mut mins: Vec<u32> = ical
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l
                .strip_prefix("TRIGGER:-P")
                .or_else(|| l.strip_prefix("TRIGGER:-p"))?;
            parse_neg_duration_minutes(rest)
        })
        .collect();
    mins.sort_unstable();
    mins.dedup();
    mins.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Parse the body of a negative iCal duration after the leading `-P`
/// (e.g. "T15M", "T1H", "1D") into total minutes. Returns None on anything
/// more exotic than a single D/H/M component.
fn parse_neg_duration_minutes(body: &str) -> Option<u32> {
    let b = body.trim().to_ascii_uppercase();
    if let Some(d) = b.strip_prefix('T') {
        // Time component: NNh or NNm.
        if let Some(h) = d.strip_suffix('H') {
            return h.parse::<u32>().ok().map(|n| n * 60);
        }
        if let Some(m) = d.strip_suffix('M') {
            return m.parse::<u32>().ok();
        }
        None
    } else if let Some(days) = b.strip_suffix('D') {
        days.parse::<u32>().ok().map(|n| n * 24 * 60)
    } else {
        None
    }
}

/// Normalize a comma-separated categories input into a single RFC 5545
/// CATEGORIES value: trim each, drop blanks, escape commas/backslashes/newlines,
/// cap at 32 to bound the line. Empty when nothing usable.
pub fn format_categories(raw: &str) -> String {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .take(32)
        .map(escape_ical)
        .collect::<Vec<_>>()
        .join(",")
}

/// Extract the emails of booked resources from an event's iCalendar: every
/// `ATTENDEE` line carrying `CUTYPE=ROOM` or `CUTYPE=RESOURCE`, read from the
/// trailing `mailto:`. Lowercased + deduped to seed the edit form's checkboxes.
pub fn booked_resources_from_ical(ical: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in ical.lines() {
        let l = line.trim();
        let upper = l.to_ascii_uppercase();
        if !upper.starts_with("ATTENDEE") {
            continue;
        }
        if !upper.contains("CUTYPE=ROOM") && !upper.contains("CUTYPE=RESOURCE") {
            continue;
        }
        if let Some(idx) = upper.rfind("MAILTO:") {
            let email = l[idx + "MAILTO:".len()..].trim().to_ascii_lowercase();
            if !email.is_empty() && !out.contains(&email) {
                out.push(email);
            }
        }
    }
    out
}

/// Extract human-attendee emails from an event's iCalendar: every `ATTENDEE`
/// line that is NOT a room/resource (`CUTYPE=ROOM`/`RESOURCE`), read from the
/// trailing `mailto:`. Lowercased + deduped. Used to re-send an iTIP invite.
pub fn attendee_emails_from_ical(ical: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in ical.lines() {
        let l = line.trim();
        let upper = l.to_ascii_uppercase();
        if !upper.starts_with("ATTENDEE") {
            continue;
        }
        if upper.contains("CUTYPE=ROOM") || upper.contains("CUTYPE=RESOURCE") {
            continue;
        }
        if let Some(idx) = upper.rfind("MAILTO:") {
            let email = l[idx + "MAILTO:".len()..].trim().to_ascii_lowercase();
            if !email.is_empty() && !out.contains(&email) {
                out.push(email);
            }
        }
    }
    out
}

/// Extract CATEGORIES from an event's iCalendar as a comma-separated string for
/// the edit form. Joins values across multiple CATEGORIES lines.
pub fn categories_from_ical(ical: &str) -> String {
    let mut cats: Vec<String> = Vec::new();
    for line in ical.lines() {
        let line = line.trim_start();
        if let Some(rest) = line
            .strip_prefix("CATEGORIES:")
            .or_else(|| line.strip_prefix("CATEGORIES;"))
        {
            // For the `;PARAM:val` shape, take after the first ':'.
            let val = rest.split_once(':').map_or(rest, |(_, v)| v);
            for c in val.split(',') {
                let c = c.trim().replace("\\,", ",").replace("\\\\", "\\");
                if !c.is_empty() {
                    cats.push(c);
                }
            }
        }
    }
    cats.join(", ")
}

/// Extract ATTACH URIs from an event's iCalendar as a newline-separated string
/// for the edit form. Skips inline binary attachments (`ATTACH;ENCODING=…`),
/// keeping only plain URI links.
pub fn attachments_from_ical(ical: &str) -> String {
    let mut uris: Vec<String> = Vec::new();
    for line in ical.lines() {
        let line = line.trim_start();
        if let Some(uri) = line.strip_prefix("ATTACH:") {
            let uri = uri.trim();
            if !uri.is_empty() {
                uris.push(uri.to_string());
            }
        }
    }
    uris.join("\n")
}

/// Build a complete VCALENDAR document for an event create/update/cancel.
/// Returns None when the start/end timestamps can't be normalized.
pub fn build_vcalendar(
    uid: &str,
    organizer_email: Option<&str>,
    attendees: &[String],
    method: Option<&str>,
    f: &EventForm,
) -> Option<String> {
    let dtstart = local_to_ical_utc(&f.dtstart)?;
    let dtend = local_to_ical_utc(&f.dtend)?;
    let now = OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .ok()?;
    let mut ical = String::new();
    ical.push_str("BEGIN:VCALENDAR\r\n");
    ical.push_str("VERSION:2.0\r\n");
    ical.push_str("PRODID:-//expresso//web//PT-BR\r\n");
    if let Some(m) = method {
        ical.push_str(&format!("METHOD:{m}\r\n"));
    }
    ical.push_str("BEGIN:VEVENT\r\n");
    ical.push_str(&format!("UID:{uid}\r\n"));
    ical.push_str(&format!("DTSTAMP:{now}\r\n"));
    ical.push_str(&format!("DTSTART:{dtstart}\r\n"));
    ical.push_str(&format!("DTEND:{dtend}\r\n"));
    if method == Some("CANCEL") {
        ical.push_str("STATUS:CANCELLED\r\n");
        ical.push_str("SEQUENCE:1\r\n");
    }
    ical.push_str(&format!("SUMMARY:{}\r\n", escape_ical(f.summary.trim())));
    if !f.location.trim().is_empty() {
        ical.push_str(&format!("LOCATION:{}\r\n", escape_ical(f.location.trim())));
    }
    if !f.description.trim().is_empty() {
        ical.push_str(&format!(
            "DESCRIPTION:{}\r\n",
            escape_ical(f.description.trim())
        ));
    }
    let cats = format_categories(&f.categories);
    if !cats.is_empty() {
        ical.push_str(&format!("CATEGORIES:{cats}\r\n"));
    }
    for uri in parse_attachment_uris(&f.attachments) {
        ical.push_str(&format!("ATTACH:{uri}\r\n"));
    }
    if let Some(email) = organizer_email {
        if !email.is_empty() {
            ical.push_str(&format!("ORGANIZER:mailto:{email}\r\n"));
        }
    }
    for a in attendees {
        ical.push_str(&format!(
            "ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:{a}\r\n"
        ));
    }
    // Booked resources (rooms/equipment): CUTYPE=ROOM marks them so the backend
    // records the booking + flags double-bookings.
    for r in parse_resource_emails(&f.resources) {
        ical.push_str(&format!(
            "ATTENDEE;CUTYPE=ROOM;ROLE=NON-PARTICIPANT;RSVP=FALSE:mailto:{r}\r\n"
        ));
    }
    // VALARMs carry reminders into the stored event (and to CalDAV clients).
    // A cancellation does not need them.
    if method != Some("CANCEL") {
        ical.push_str(&build_valarms(&f.reminders, f.summary.trim()));
    }
    ical.push_str("END:VEVENT\r\n");
    ical.push_str("END:VCALENDAR\r\n");
    Some(ical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_categories_trims_and_filters() {
        assert_eq!(format_categories(" work , , personal "), "work,personal");
        assert_eq!(format_categories(""), "");
        assert_eq!(format_categories("  ,  ,  "), "");
        // a semicolon within a category is escaped (it's not a separator here)
        assert_eq!(format_categories("a;b"), "a\\;b");
    }

    #[test]
    fn categories_from_ical_extracts_and_joins() {
        let ical = "BEGIN:VEVENT\r\nCATEGORIES:work,personal\r\nEND:VEVENT\r\n";
        assert_eq!(categories_from_ical(ical), "work, personal");
        assert_eq!(categories_from_ical("CATEGORIES:solo\r\n"), "solo");
        assert_eq!(categories_from_ical("SUMMARY:x\r\n"), "");
    }

    #[test]
    fn parse_attachment_uris_keeps_only_known_schemes() {
        let v = parse_attachment_uris("https://a/x\nhttp://b\nnotaurl\nftp://c\nmailto:d@e");
        assert_eq!(v, vec!["https://a/x", "http://b", "mailto:d@e"]);
        assert!(parse_attachment_uris("  , , ").is_empty());
    }

    #[test]
    fn attachments_from_ical_extracts_uri_lines() {
        let ical = "BEGIN:VEVENT\r\nATTACH:https://a/x\r\nATTACH:https://b/y\r\nSUMMARY:s\r\n";
        assert_eq!(attachments_from_ical(ical), "https://a/x\nhttps://b/y");
        assert_eq!(attachments_from_ical("SUMMARY:x\r\n"), "");
    }

    #[test]
    fn build_valarms_emits_one_block_per_minute() {
        let out = build_valarms("15,60", "Reunião");
        assert_eq!(out.matches("BEGIN:VALARM").count(), 2);
        assert!(out.contains("TRIGGER:-PT15M"));
        assert!(out.contains("TRIGGER:-PT60M"));
        assert!(out.contains("ACTION:DISPLAY"));
    }

    #[test]
    fn build_valarms_dedups_sorts_and_skips_junk() {
        let out = build_valarms("60, 15, 15, x, -5", "x");
        // 15 and 60 survive (dedup); junk and negative dropped.
        assert_eq!(out.matches("BEGIN:VALARM").count(), 2);
        let p15 = out.find("PT15M").unwrap();
        let p60 = out.find("PT60M").unwrap();
        assert!(p15 < p60, "sorted ascending");
    }

    #[test]
    fn build_valarms_empty_is_empty() {
        assert_eq!(build_valarms("", "x"), "");
        assert_eq!(build_valarms("  ,  ", "x"), "");
    }

    #[test]
    fn valarm_minutes_extracts_from_ical() {
        let ical = "BEGIN:VALARM\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n\
                    BEGIN:VALARM\r\nTRIGGER:-PT1H\r\nEND:VALARM\r\n";
        assert_eq!(valarm_minutes(ical), "15,60");
    }

    #[test]
    fn valarm_minutes_handles_days_and_dedup() {
        let ical = "TRIGGER:-P1D\r\nTRIGGER:-PT60M\r\nTRIGGER:-PT60M\r\n";
        assert_eq!(valarm_minutes(ical), "60,1440");
    }

    #[test]
    fn valarm_minutes_empty_when_none() {
        assert_eq!(valarm_minutes("BEGIN:VEVENT\r\nEND:VEVENT\r\n"), "");
    }

    #[test]
    fn parse_neg_duration_minutes_variants() {
        assert_eq!(parse_neg_duration_minutes("T15M"), Some(15));
        assert_eq!(parse_neg_duration_minutes("T2H"), Some(120));
        assert_eq!(parse_neg_duration_minutes("1D"), Some(1440));
        assert_eq!(parse_neg_duration_minutes("T1H30M"), None); // compound unsupported
        assert_eq!(parse_neg_duration_minutes("xyz"), None);
    }

    #[test]
    fn build_vcalendar_emits_core_fields() {
        let f = EventForm {
            summary: "Reunião".into(),
            location: "Sala 1".into(),
            description: String::new(),
            dtstart: "2026-06-10T09:00".into(),
            dtend: "2026-06-10T10:00".into(),
            attendees: String::new(),
            reminders: "15".into(),
            categories: "work".into(),
            resources: String::new(),
            attachments: String::new(),
        };
        let ical = build_vcalendar("uid-1@x", Some("a@ex.com"), &["b@ex.com".into()], None, &f)
            .expect("valid dates");
        assert!(ical.contains("BEGIN:VCALENDAR"));
        assert!(ical.contains("UID:uid-1@x"));
        assert!(ical.contains("DTSTART:20260610T090000Z"));
        assert!(ical.contains("SUMMARY:Reunião"));
        assert!(ical.contains("LOCATION:Sala 1"));
        assert!(ical.contains("CATEGORIES:work"));
        assert!(ical.contains("ORGANIZER:mailto:a@ex.com"));
        assert!(ical.contains("ATTENDEE;ROLE=REQ-PARTICIPANT"));
        assert!(ical.contains("TRIGGER:-PT15M"));
    }

    #[test]
    fn build_vcalendar_cancel_has_status_no_alarms() {
        let f = EventForm {
            summary: "x".into(),
            location: String::new(),
            description: String::new(),
            dtstart: "2026-06-10T09:00".into(),
            dtend: "2026-06-10T10:00".into(),
            attendees: String::new(),
            reminders: "15".into(),
            categories: String::new(),
            resources: String::new(),
            attachments: String::new(),
        };
        let ical = build_vcalendar("u", None, &[], Some("CANCEL"), &f).expect("valid dates");
        assert!(ical.contains("METHOD:CANCEL"));
        assert!(ical.contains("STATUS:CANCELLED"));
        assert!(!ical.contains("BEGIN:VALARM"));
    }

    #[test]
    fn to_rfc3339_adds_seconds_and_utc() {
        assert_eq!(
            to_rfc3339("2026-06-10T14:30"),
            Some("2026-06-10T14:30:00Z".into())
        );
    }

    #[test]
    fn to_rfc3339_passes_through_offset_and_z() {
        assert_eq!(
            to_rfc3339("2026-06-10T14:30:00Z"),
            Some("2026-06-10T14:30:00Z".into())
        );
        assert_eq!(
            to_rfc3339("2026-06-10T14:30:00-03:00"),
            Some("2026-06-10T14:30:00-03:00".into())
        );
    }

    #[test]
    fn to_rfc3339_blank_is_none() {
        assert_eq!(to_rfc3339(""), None);
        assert_eq!(to_rfc3339("   "), None);
    }

    #[test]
    fn build_vcalendar_rejects_bad_dates() {
        let f = EventForm {
            summary: "x".into(),
            location: String::new(),
            description: String::new(),
            dtstart: "not-a-date".into(),
            dtend: "2026-06-10T10:00".into(),
            attendees: String::new(),
            reminders: String::new(),
            categories: String::new(),
            resources: String::new(),
            attachments: String::new(),
        };
        assert!(build_vcalendar("u", None, &[], None, &f).is_none());
    }

    #[test]
    fn parse_resource_emails_dedups_lowercases_filters() {
        assert_eq!(
            parse_resource_emails("Sala1@x.com, sala1@x.com; notanemail , sala2@x.com"),
            vec!["sala1@x.com", "sala2@x.com"]
        );
        assert!(parse_resource_emails("").is_empty());
        assert!(parse_resource_emails("nope").is_empty());
    }

    #[test]
    fn build_vcalendar_emits_room_attendee_for_resources() {
        let f = EventForm {
            summary: "Reunião".into(),
            location: String::new(),
            description: String::new(),
            dtstart: "2026-06-10T09:00".into(),
            dtend: "2026-06-10T10:00".into(),
            attendees: String::new(),
            reminders: String::new(),
            categories: String::new(),
            resources: "sala1@x.com".into(),
            attachments: String::new(),
        };
        let ical = build_vcalendar("u", None, &[], None, &f).expect("valid");
        assert!(ical
            .contains("ATTENDEE;CUTYPE=ROOM;ROLE=NON-PARTICIPANT;RSVP=FALSE:mailto:sala1@x.com"));
    }

    #[test]
    fn booked_resources_from_ical_extracts_room_attendees() {
        let ical = "BEGIN:VEVENT\r\n\
                    ATTENDEE;ROLE=REQ-PARTICIPANT:mailto:bob@x.com\r\n\
                    ATTENDEE;CUTYPE=ROOM;ROLE=NON-PARTICIPANT:mailto:Sala1@x.com\r\n\
                    ATTENDEE;CUTYPE=RESOURCE:mailto:projetor@x.com\r\n\
                    END:VEVENT\r\n";
        let r = booked_resources_from_ical(ical);
        assert_eq!(r, vec!["sala1@x.com", "projetor@x.com"]);
        // a plain attendee is not a resource
        assert!(!r.contains(&"bob@x.com".to_string()));
    }

    #[test]
    fn attendee_emails_skips_rooms_and_dedups() {
        let ical = "BEGIN:VEVENT\r\n\
                    ATTENDEE;ROLE=REQ-PARTICIPANT:mailto:Bob@x.com\r\n\
                    ATTENDEE;ROLE=REQ-PARTICIPANT:mailto:bob@x.com\r\n\
                    ATTENDEE;CUTYPE=ROOM:mailto:sala1@x.com\r\n\
                    ATTENDEE;CUTYPE=RESOURCE:mailto:projetor@x.com\r\n\
                    ATTENDEE:mailto:ana@x.com\r\n\
                    END:VEVENT\r\n";
        let a = attendee_emails_from_ical(ical);
        assert_eq!(a, vec!["bob@x.com", "ana@x.com"]);
    }
}

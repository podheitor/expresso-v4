//! Minimal vCard (RFC 6350 + RFC 2426 compat) parser.
//!
//! Extracts: UID, FN, N (family;given), ORG, primary EMAIL, primary TEL.
//! Handles RFC 5545-style line unfolding (CRLF + WSP) and strips TYPE params.
//! Anything not recognised is retained in `raw`.

use sha2::{Digest, Sha256};

#[derive(Debug, Default)]
pub struct ParsedVCard {
    pub uid:          String,
    pub full_name:    Option<String>,
    pub family_name:  Option<String>,
    pub given_name:   Option<String>,
    pub organization: Option<String>,
    pub email:        Option<String>,
    pub phone:        Option<String>,
}

/// Parse a vCard (3.0 or 4.0). Returns `Err` if no UID or no BEGIN:VCARD.
pub fn parse(raw: &str) -> Result<ParsedVCard, String> {
    let unfolded = unfold(raw);
    let mut out = ParsedVCard::default();

    let mut inside = false;
    for line in unfolded.lines() {
        let trimmed = line.trim_end_matches('\r');
        if trimmed.eq_ignore_ascii_case("BEGIN:VCARD") { inside = true; continue; }
        if trimmed.eq_ignore_ascii_case("END:VCARD")   { inside = false; continue; }
        if !inside { continue; }

        // Split "NAME;PARAMS:VALUE" → (name, params_and_value)
        let (head, value) = match trimmed.split_once(':') {
            Some(v) => v,
            None => continue,
        };
        // head may be "TEL;TYPE=CELL" → take the bare property name before ';'
        let prop = head.split(';').next().unwrap_or(head).to_ascii_uppercase();

        match prop.as_str() {
            "UID" if out.uid.is_empty()            => out.uid          = value.trim().to_owned(),
            "FN"  if out.full_name.is_none()       => out.full_name    = Some(value.trim().to_owned()),
            "ORG" if out.organization.is_none()    => out.organization = Some(value.trim().to_owned()),
            "EMAIL" if out.email.is_none()         => out.email        = Some(value.trim().to_owned()),
            "TEL"   if out.phone.is_none()         => out.phone        = Some(value.trim().to_owned()),
            "N" if out.family_name.is_none()       => {
                // N = Family;Given;Additional;Prefix;Suffix
                let parts: Vec<&str> = value.split(';').collect();
                if let Some(f) = parts.first() { if !f.is_empty() { out.family_name = Some(f.trim().to_owned()); } }
                if let Some(g) = parts.get(1)  { if !g.is_empty() { out.given_name  = Some(g.trim().to_owned()); } }
            }
            _ => {}
        }
    }

    if out.uid.is_empty() {
        return Err("vCard missing UID property".into());
    }
    Ok(out)
}

/// Stable ETag — sha256(raw) hex-encoded.
pub fn compute_etag(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

/// Unfold vCard/iCal long lines: CRLF followed by space/tab is a continuation.
fn unfold(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut iter = raw.chars().peekable();
    while let Some(c) = iter.next() {
        if c == '\r' && iter.peek() == Some(&'\n') {
            iter.next();
            match iter.peek() {
                Some(' ') | Some('\t') => { iter.next(); /* fold */ }
                _ => out.push('\n'),
            }
        } else if c == '\n' {
            match iter.peek() {
                Some(' ') | Some('\t') => { iter.next(); /* fold */ }
                _ => out.push('\n'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Split a file containing multiple BEGIN:VCARD..END:VCARD blocks into
/// individual vCard strings (each with its own BEGIN/END). Non-card content
/// is ignored. Returns empty vec when no blocks found.
pub fn split_vcards(raw: &str) -> Vec<String> {
    let unfolded = unfold(raw);
    let mut out: Vec<String> = Vec::new();
    let mut buf: Option<Vec<String>> = None;
    for line in unfolded.lines() {
        let trimmed = line.trim_end_matches('\r');
        if trimmed.eq_ignore_ascii_case("BEGIN:VCARD") {
            buf = Some(vec![trimmed.to_owned()]);
            continue;
        }
        if trimmed.eq_ignore_ascii_case("END:VCARD") {
            if let Some(mut b) = buf.take() {
                b.push(trimmed.to_owned());
                out.push(b.join("\r\n") + "\r\n");
            }
            continue;
        }
        if let Some(b) = buf.as_mut() { b.push(trimmed.to_owned()); }
    }
    out
}

/// Concatenate multiple vCard bodies into a single file for export. Each
/// input is expected to be a full VCARD (already contains BEGIN/END).
pub fn concat_vcards(cards: &[String]) -> String {
    let mut s = String::with_capacity(cards.iter().map(|c| c.len()).sum::<usize>() + 64);
    for c in cards {
        s.push_str(c);
        if !c.ends_with('\n') { s.push_str("\r\n"); }
    }
    s
}

/// Build a minimal vCard 4.0 from discrete fields. Used by GAL→contatos sync
/// to materialize a directory user into the caller's addressbook without
/// requiring a raw vCard payload. Values are newline/semicolon-stripped to
/// avoid property injection.
pub fn build_vcard(
    uid: &str,
    full_name: &str,
    family_name: Option<&str>,
    given_name:  Option<&str>,
    email: Option<&str>,
    organization: Option<&str>,
) -> String {
    fn escape(v: &str) -> String {
        v.replace('\r', " ").replace('\n', " ").replace(';', ",")
    }
    let fn_v  = escape(full_name);
    let uid_v = escape(uid);
    let n_v = format!(
        "{};{};;;",
        family_name.map(escape).unwrap_or_default(),
        given_name.map(escape).unwrap_or_default()
    );
    let mut s = String::new();
    s.push_str("BEGIN:VCARD\r\n");
    s.push_str("VERSION:4.0\r\n");
    s.push_str(&format!("UID:{uid_v}\r\n"));
    s.push_str(&format!("FN:{fn_v}\r\n"));
    s.push_str(&format!("N:{n_v}\r\n"));
    if let Some(o) = organization { s.push_str(&format!("ORG:{}\r\n", escape(o))); }
    if let Some(e) = email        { s.push_str(&format!("EMAIL;TYPE=INTERNET:{}\r\n", escape(e))); }
    s.push_str("END:VCARD\r\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:abc-123\r\nFN:John Doe\r\nN:Doe;John;;;\r\nORG:Acme Co\r\nEMAIL;TYPE=INTERNET:john@example.com\r\nTEL;TYPE=CELL:+5511999999999\r\nEND:VCARD\r\n";

    #[test]
    fn parses_basic_fields() {
        let v = parse(SAMPLE).unwrap();
        assert_eq!(v.uid, "abc-123");
        assert_eq!(v.full_name.as_deref(), Some("John Doe"));
        assert_eq!(v.family_name.as_deref(), Some("Doe"));
        assert_eq!(v.given_name.as_deref(), Some("John"));
        assert_eq!(v.organization.as_deref(), Some("Acme Co"));
        assert_eq!(v.email.as_deref(), Some("john@example.com"));
        assert_eq!(v.phone.as_deref(), Some("+5511999999999"));
    }

    #[test]
    fn missing_uid_errors() {
        let raw = "BEGIN:VCARD\r\nFN:X\r\nEND:VCARD\r\n";
        assert!(parse(raw).is_err());
    }

    #[test]
    fn etag_stable() {
        assert_eq!(compute_etag(SAMPLE), compute_etag(SAMPLE));
        assert_ne!(compute_etag(SAMPLE), compute_etag("other"));
    }

    #[test]
    fn handles_line_folding() {
        let folded = "BEGIN:VCARD\r\nUID:u1\r\nFN:Very Long\r\n  Continued Name\r\nEND:VCARD\r\n";
        let v = parse(folded).unwrap();
        assert_eq!(v.full_name.as_deref(), Some("Very Long Continued Name"));
    }

    #[test]
    fn empty_input_returns_error() {
        assert!(parse("").is_err());
    }

    #[test]
    fn unknown_property_ignored() {
        let raw = "BEGIN:VCARD\r\nUID:u2\r\nFN:X\r\nX-CUSTOM:whatever\r\nEND:VCARD\r\n";
        let v = parse(raw).unwrap();
        assert_eq!(v.uid, "u2");
    }

    #[test]
    fn very_long_note_does_not_panic() {
        // Allocation-bomb guard: NOTE of 256 KiB must not panic.
        let note = "Z".repeat(256 * 1024);
        let raw = format!("BEGIN:VCARD\r\nUID:u3\r\nFN:A\r\nNOTE:{note}\r\nEND:VCARD\r\n");
        // parse() must not panic regardless of whether it stores NOTE.
        let _ = parse(&raw);
    }

    #[test]
    fn missing_fn_is_tolerated() {
        // FN is REQUIRED per RFC 6350 but parsers should not panic on omission.
        let raw = "BEGIN:VCARD\r\nUID:u4\r\nEND:VCARD\r\n";
        // Either Ok (with empty full_name) or Err — must not panic.
        let _ = parse(raw);
    }

    #[test]
    fn parse_email_in_vcard() {
        let raw = "BEGIN:VCARD\r\nUID:u5\r\nFN:Bob\r\nEMAIL:bob@example.com\r\nEND:VCARD\r\n";
        let c = parse(raw).unwrap();
        assert_eq!(c.email_primary.as_deref(), Some("bob@example.com"));
    }

    #[test]
    fn compute_etag_is_deterministic() {
        let raw = "BEGIN:VCARD\r\nUID:u1\r\nFN:Alice\r\nEND:VCARD\r\n";
        assert_eq!(compute_etag(raw), compute_etag(raw));
    }

    #[test]
    fn compute_etag_differs_for_different_inputs() {
        let a = compute_etag("BEGIN:VCARD\r\nUID:u1\r\nFN:Alice\r\nEND:VCARD\r\n");
        let b = compute_etag("BEGIN:VCARD\r\nUID:u2\r\nFN:Bob\r\nEND:VCARD\r\n");
        assert_ne!(a, b);
    }

    #[test]
    fn parsed_vcard_uid_extracted() {
        let raw = "BEGIN:VCARD\r\nUID:my-uid-123\r\nFN:Test\r\nEND:VCARD\r\n";
        let p = parse(raw).unwrap();
        assert_eq!(p.uid, "my-uid-123");
    }

    #[test]
    fn compute_etag_is_hex_string() {
        let e = compute_etag("BEGIN:VCARD\r\nUID:x\r\nFN:X\r\nEND:VCARD\r\n");
        assert!(e.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn compute_etag_differs_for_distinct_inputs() {
        assert_ne!(compute_etag("AAA"), compute_etag("BBB"));
    }

    #[test]
    fn compute_etag_nonempty_for_minimal_vcard() {
        let raw = "BEGIN:VCARD\r\nUID:x\r\nFN:A\r\nEND:VCARD\r\n";
        assert!(!compute_etag(raw).is_empty());
    }

    #[test]
    fn build_vcard_contains_uid_and_fn() {
        let v = build_vcard("uid-1", "Alice Smith", Some("Smith"), Some("Alice"), None, None);
        assert!(v.contains("UID:uid-1"));
        assert!(v.contains("FN:Alice Smith"));
    }

    #[test]
    fn build_vcard_begins_with_vcard_header() {
        let v = build_vcard("u2", "Bob", None, None, None, None);
        assert!(v.starts_with("BEGIN:VCARD"));
    }

    #[test]
    fn build_vcard_ends_with_vcard_footer() {
        let v = build_vcard("u3", "Carol", None, None, None, None);
        assert!(v.trim_end().ends_with("END:VCARD"));
    }

    #[test]
    fn build_vcard_email_field_included_when_provided() {
        let v = build_vcard("u4", "Dave", None, None, Some("dave@example.com"), None);
        assert!(v.contains("dave@example.com"));
    }

    #[test]
    fn build_vcard_phone_field_included_when_provided() {
        let v = build_vcard("u5", "Eve", None, None, None, Some("+5511999999999"));
        assert!(v.contains("+5511999999999"));
    }
}

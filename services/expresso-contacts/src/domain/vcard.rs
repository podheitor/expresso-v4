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
}

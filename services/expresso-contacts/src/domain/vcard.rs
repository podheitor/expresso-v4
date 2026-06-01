//! Minimal vCard (RFC 6350 + RFC 2426 compat) parser.
//!
//! Extracts: UID, FN, N (family;given), ORG, primary EMAIL, primary TEL.
//! Handles RFC 5545-style line unfolding (CRLF + WSP) and strips TYPE params.
//! Anything not recognised is retained in `raw`.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};

/// A contact PHOTO (RFC 6350 §6.2.4). Either an external URI reference, or
/// inline image bytes with their media type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Photo {
    /// `PHOTO:https://…` / `PHOTO;MEDIATYPE=image/jpeg:https://…`.
    Uri(String),
    /// Decoded inline image: `(media_type, bytes)`. Media type defaults to
    /// `application/octet-stream` when the vCard omits TYPE/MEDIATYPE.
    Inline { media_type: String, bytes: Vec<u8> },
}

/// One parsed EMAIL property: the address plus its TYPE label (e.g. "WORK",
/// "HOME"), lowercased; `None` when the vCard gives no TYPE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactEmail {
    pub address: String,
    pub label: Option<String>,
}

/// One parsed ADR property (RFC 6350 §6.3.1). The structured value is 7
/// `;`-separated components: po-box, ext, street, locality, region, postal,
/// country. Empty components stay `None`. `label` is the lowercased TYPE.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContactAddress {
    pub label: Option<String>,
    pub po_box: Option<String>,
    pub ext: Option<String>,
    pub street: Option<String>,
    pub locality: Option<String>,
    pub region: Option<String>,
    pub postal_code: Option<String>,
    pub country: Option<String>,
}

#[derive(Debug, Default)]
pub struct ParsedVCard {
    pub uid: String,
    pub full_name: Option<String>,
    pub family_name: Option<String>,
    pub given_name: Option<String>,
    pub organization: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    /// Every EMAIL in the vCard (incl. the primary), in document order.
    pub emails: Vec<ContactEmail>,
    /// Every ADR in the vCard, in document order.
    pub addresses: Vec<ContactAddress>,
    /// BDAY value verbatim (RFC 6350 §6.2.5) — may be a full date or a partial
    /// `--MMDD`. None when absent.
    pub birthday: Option<String>,
    /// First NICKNAME value (RFC 6350 §6.2.3), comma-list collapsed to its head.
    pub nickname: Option<String>,
}

/// Parse a vCard (3.0 or 4.0). Returns `Err` if no UID or no BEGIN:VCARD.
pub fn parse(raw: &str) -> Result<ParsedVCard, String> {
    let unfolded = unfold(raw);
    let mut out = ParsedVCard::default();

    let mut inside = false;
    for line in unfolded.lines() {
        let trimmed = line.trim_end_matches('\r');
        if trimmed.eq_ignore_ascii_case("BEGIN:VCARD") {
            inside = true;
            continue;
        }
        if trimmed.eq_ignore_ascii_case("END:VCARD") {
            inside = false;
            continue;
        }
        if !inside {
            continue;
        }

        // Split "NAME;PARAMS:VALUE" → (name, params_and_value)
        let (head, value) = match trimmed.split_once(':') {
            Some(v) => v,
            None => continue,
        };
        // head may be "TEL;TYPE=CELL" → take the bare property name before ';'
        let prop = head.split(';').next().unwrap_or(head).to_ascii_uppercase();

        match prop.as_str() {
            "UID" if out.uid.is_empty() => out.uid = value.trim().to_owned(),
            "FN" if out.full_name.is_none() => out.full_name = Some(value.trim().to_owned()),
            "ORG" if out.organization.is_none() => out.organization = Some(value.trim().to_owned()),
            "EMAIL" => {
                let addr = value.trim();
                if !addr.is_empty() {
                    // First EMAIL also fills the denormalized primary column.
                    if out.email.is_none() {
                        out.email = Some(addr.to_owned());
                    }
                    out.emails.push(ContactEmail {
                        address: addr.to_owned(),
                        label: email_type_label(head),
                    });
                }
            }
            "TEL" if out.phone.is_none() => out.phone = Some(value.trim().to_owned()),
            "ADR" => {
                if let Some(adr) = parse_adr(head, value) {
                    out.addresses.push(adr);
                }
            }
            "BDAY" if out.birthday.is_none() => {
                let b = value.trim();
                if !b.is_empty() {
                    out.birthday = Some(b.to_owned());
                }
            }
            "NICKNAME" if out.nickname.is_none() => {
                // NICKNAME is a comma-separated list; index the first entry.
                let n = value.split(',').next().unwrap_or(value).trim();
                if !n.is_empty() {
                    out.nickname = Some(n.to_owned());
                }
            }
            "N" if out.family_name.is_none() => {
                // N = Family;Given;Additional;Prefix;Suffix
                let parts: Vec<&str> = value.split(';').collect();
                if let Some(f) = parts.first() {
                    if !f.is_empty() {
                        out.family_name = Some(f.trim().to_owned());
                    }
                }
                if let Some(g) = parts.get(1) {
                    if !g.is_empty() {
                        out.given_name = Some(g.trim().to_owned());
                    }
                }
            }
            _ => {}
        }
    }

    if out.uid.is_empty() {
        return Err("vCard missing UID property".into());
    }
    Ok(out)
}

/// Extract the first PHOTO property from a vCard, resolving its form (external
/// URI vs inline base64). Returns `None` when there is no PHOTO or the inline
/// payload can't be decoded. Handles the vCard 3.0 (`ENCODING=b`/`BASE64`) and
/// 4.0 (`data:` URI, or bare URI) variants.
pub fn parse_photo(raw: &str) -> Option<Photo> {
    let unfolded = unfold(raw);
    for line in unfolded.lines() {
        let trimmed = line.trim_end_matches('\r');
        let Some((head, value)) = trimmed.split_once(':') else {
            continue;
        };
        let name = head.split(';').next().unwrap_or(head);
        if !name.eq_ignore_ascii_case("PHOTO") {
            continue;
        }
        return interpret_photo(head, value.trim());
    }
    None
}

/// Extract a lowercased TYPE label from an EMAIL property head
/// (`EMAIL;TYPE=WORK` → `Some("work")`). Handles `TYPE=` and bare type tokens
/// (vCard 3.0 `EMAIL;INTERNET`). None when no usable label is present.
fn email_type_label(head: &str) -> Option<String> {
    head.split(';').skip(1).find_map(|p| {
        let p = p.trim();
        let val = p.strip_prefix("TYPE=").or_else(|| p.strip_prefix("type="));
        let token = val.unwrap_or(p);
        // Skip empty and the ubiquitous "INTERNET"/"PREF" noise.
        let t = token.trim().to_ascii_lowercase();
        if t.is_empty() || t == "internet" || t == "pref" {
            None
        } else {
            Some(t)
        }
    })
}

/// Parse an ADR property (`head` = `ADR[;params]`, `value` = the 7 `;`-separated
/// structured components). Returns `None` when every component is empty. Each
/// component is text-unescaped, trimmed; empties become `None`.
fn parse_adr(head: &str, value: &str) -> Option<ContactAddress> {
    let parts: Vec<Option<String>> = value
        .split(';')
        .map(|c| {
            let t = c.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_owned())
            }
        })
        .collect();
    let get = |i: usize| parts.get(i).cloned().flatten();
    let adr = ContactAddress {
        label: email_type_label(head), // same TYPE extraction as EMAIL
        po_box: get(0),
        ext: get(1),
        street: get(2),
        locality: get(3),
        region: get(4),
        postal_code: get(5),
        country: get(6),
    };
    // Skip an ADR with no usable component (e.g. a bare "ADR:;;;;;;").
    let empty = adr.po_box.is_none()
        && adr.ext.is_none()
        && adr.street.is_none()
        && adr.locality.is_none()
        && adr.region.is_none()
        && adr.postal_code.is_none()
        && adr.country.is_none();
    if empty {
        None
    } else {
        Some(adr)
    }
}

/// Resolve a PHOTO `head` (property + params) and `value` into a [`Photo`].
fn interpret_photo(head: &str, value: &str) -> Option<Photo> {
    let params: Vec<&str> = head.split(';').skip(1).collect();
    let is_b64 = params.iter().any(|p| {
        let p = p.to_ascii_uppercase();
        p == "ENCODING=B" || p == "ENCODING=BASE64" || p == "BASE64"
    });

    // vCard 4.0 inline data URI: `PHOTO:data:image/png;base64,AAAA…`.
    if let Some(rest) = value.strip_prefix("data:") {
        return decode_data_uri(rest);
    }
    if is_b64 {
        let media_type = photo_media_type(&params);
        let bytes = STANDARD.decode(value.replace([' ', '\t'], "")).ok()?;
        return Some(Photo::Inline { media_type, bytes });
    }
    // Otherwise it's a URI reference (http/https/etc.).
    if value.is_empty() {
        return None;
    }
    Some(Photo::Uri(value.to_owned()))
}

/// Decode a `data:` URI body (`<media>;base64,<b64>` or `<media>,<urlenc>`).
/// Only the base64 form is supported (the common PHOTO encoding).
fn decode_data_uri(rest: &str) -> Option<Photo> {
    let (meta, data) = rest.split_once(',')?;
    if !meta.to_ascii_lowercase().contains("base64") {
        return None;
    }
    let media_type = meta
        .split(';')
        .next()
        .filter(|m| !m.is_empty())
        .map_or_else(
            || "application/octet-stream".to_owned(),
            |m| m.to_ascii_lowercase(),
        );
    let bytes = STANDARD.decode(data.replace([' ', '\t'], "")).ok()?;
    Some(Photo::Inline { media_type, bytes })
}

/// Derive an image media type from a vCard 3.0 `TYPE=JPEG` param, or fall back
/// to a generic binary type.
fn photo_media_type(params: &[&str]) -> String {
    for p in params {
        if let Some(t) = p.to_ascii_uppercase().strip_prefix("TYPE=") {
            if !t.is_empty() {
                return format!("image/{}", t.to_ascii_lowercase());
            }
        }
        if let Some(m) = p.to_ascii_uppercase().strip_prefix("MEDIATYPE=") {
            if !m.is_empty() {
                return m.to_ascii_lowercase();
            }
        }
    }
    "application/octet-stream".to_owned()
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
                Some(' ') | Some('\t') => {
                    iter.next(); /* fold */
                }
                _ => out.push('\n'),
            }
        } else if c == '\n' {
            match iter.peek() {
                Some(' ') | Some('\t') => {
                    iter.next(); /* fold */
                }
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
        if let Some(b) = buf.as_mut() {
            b.push(trimmed.to_owned());
        }
    }
    out
}

/// Concatenate multiple vCard bodies into a single file for export. Each
/// input is expected to be a full VCARD (already contains BEGIN/END).
pub fn concat_vcards(cards: &[String]) -> String {
    let mut s = String::with_capacity(cards.iter().map(|c| c.len()).sum::<usize>() + 64);
    for c in cards {
        s.push_str(c);
        if !c.ends_with('\n') {
            s.push_str("\r\n");
        }
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
    given_name: Option<&str>,
    email: Option<&str>,
    organization: Option<&str>,
) -> String {
    fn escape(v: &str) -> String {
        v.replace(['\r', '\n'], " ").replace(';', ",")
    }
    let fn_v = escape(full_name);
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
    if let Some(o) = organization {
        s.push_str(&format!("ORG:{}\r\n", escape(o)));
    }
    if let Some(e) = email {
        s.push_str(&format!("EMAIL;TYPE=INTERNET:{}\r\n", escape(e)));
    }
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
        assert_eq!(c.email.as_deref(), Some("bob@example.com"));
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
        let v = build_vcard(
            "uid-1",
            "Alice Smith",
            Some("Smith"),
            Some("Alice"),
            None,
            None,
        );
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

    #[test]
    fn build_vcard_org_field_included_when_provided() {
        let v = build_vcard("u6", "Frank", None, None, None, Some("ACME Corp"));
        assert!(v.contains("ACME Corp"));
    }

    #[test]
    fn build_vcard_contains_version_field() {
        let v = build_vcard("u7", "Grace", None, None, None, None);
        assert!(v.contains("VERSION:"));
    }

    #[test]
    fn build_vcard_uid_present_in_output() {
        let v = build_vcard("test-uid-42", "Alice", None, None, None, None);
        assert!(v.contains("test-uid-42"));
    }

    // ---- ADR ----

    #[test]
    fn parses_adr_structured_components() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:a1\r\nFN:A\r\nADR;TYPE=WORK:;;123 Main St;Springfield;IL;62704;USA\r\nEND:VCARD\r\n";
        let c = parse(raw).unwrap();
        assert_eq!(c.addresses.len(), 1);
        let a = &c.addresses[0];
        assert_eq!(a.label.as_deref(), Some("work"));
        assert_eq!(a.po_box, None);
        assert_eq!(a.street.as_deref(), Some("123 Main St"));
        assert_eq!(a.locality.as_deref(), Some("Springfield"));
        assert_eq!(a.region.as_deref(), Some("IL"));
        assert_eq!(a.postal_code.as_deref(), Some("62704"));
        assert_eq!(a.country.as_deref(), Some("USA"));
    }

    #[test]
    fn skips_fully_empty_adr() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:a2\r\nFN:B\r\nADR:;;;;;;\r\nEND:VCARD\r\n";
        let c = parse(raw).unwrap();
        assert!(c.addresses.is_empty());
    }

    #[test]
    fn parses_multiple_adr_in_order() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:a3\r\nFN:C\r\nADR;TYPE=HOME:;;1 First Ave;;;;\r\nADR;TYPE=WORK:;;2 Second St;;;;\r\nEND:VCARD\r\n";
        let c = parse(raw).unwrap();
        assert_eq!(c.addresses.len(), 2);
        assert_eq!(c.addresses[0].label.as_deref(), Some("home"));
        assert_eq!(c.addresses[1].street.as_deref(), Some("2 Second St"));
    }

    // ---- BDAY / NICKNAME ----

    #[test]
    fn parses_all_emails_with_labels() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:e1\r\nFN:Multi\r\nEMAIL;TYPE=WORK:w@x.com\r\nEMAIL;TYPE=HOME:h@x.com\r\nEMAIL;TYPE=INTERNET:plain@x.com\r\nEND:VCARD\r\n";
        let c = parse(raw).unwrap();
        // Primary column = first address.
        assert_eq!(c.email.as_deref(), Some("w@x.com"));
        assert_eq!(c.emails.len(), 3);
        assert_eq!(c.emails[0].address, "w@x.com");
        assert_eq!(c.emails[0].label.as_deref(), Some("work"));
        assert_eq!(c.emails[1].label.as_deref(), Some("home"));
        // INTERNET is noise → no label.
        assert_eq!(c.emails[2].label, None);
    }

    #[test]
    fn email_label_skips_noise_tokens() {
        assert_eq!(email_type_label("EMAIL;TYPE=WORK"), Some("work".into()));
        assert_eq!(email_type_label("EMAIL;TYPE=INTERNET"), None);
        assert_eq!(email_type_label("EMAIL;PREF"), None);
        assert_eq!(email_type_label("EMAIL"), None);
        assert_eq!(email_type_label("EMAIL;HOME"), Some("home".into()));
    }

    #[test]
    fn parses_bday_and_nickname() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:b1\r\nFN:Ada\r\nBDAY:1990-05-15\r\nNICKNAME:Addie,Ace\r\nEND:VCARD\r\n";
        let c = parse(raw).unwrap();
        assert_eq!(c.birthday.as_deref(), Some("1990-05-15"));
        // NICKNAME is a comma-list; we keep the first.
        assert_eq!(c.nickname.as_deref(), Some("Addie"));
    }

    #[test]
    fn bday_partial_date_preserved_verbatim() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:b2\r\nFN:Y\r\nBDAY:--0515\r\nEND:VCARD\r\n";
        let c = parse(raw).unwrap();
        assert_eq!(c.birthday.as_deref(), Some("--0515"));
        assert!(c.nickname.is_none());
    }

    // ---- PHOTO ----

    /// base64 of the 3 bytes 0x01 0x02 0x03 = "AQID".
    const B64_3: &str = "AQID";

    #[test]
    fn parse_photo_none_when_absent() {
        assert!(parse_photo(SAMPLE).is_none());
    }

    #[test]
    fn parse_photo_uri_reference() {
        let raw = "BEGIN:VCARD\r\nUID:u\r\nFN:A\r\nPHOTO;MEDIATYPE=image/jpeg:https://x.org/p.jpg\r\nEND:VCARD\r\n";
        assert_eq!(
            parse_photo(raw),
            Some(Photo::Uri("https://x.org/p.jpg".into()))
        );
    }

    #[test]
    fn parse_photo_vcard3_inline_base64() {
        let raw =
            format!("BEGIN:VCARD\r\nUID:u\r\nPHOTO;ENCODING=b;TYPE=PNG:{B64_3}\r\nEND:VCARD\r\n");
        assert_eq!(
            parse_photo(&raw),
            Some(Photo::Inline {
                media_type: "image/png".into(),
                bytes: vec![1, 2, 3],
            })
        );
    }

    #[test]
    fn parse_photo_vcard4_data_uri() {
        let raw =
            format!("BEGIN:VCARD\r\nUID:u\r\nPHOTO:data:image/gif;base64,{B64_3}\r\nEND:VCARD\r\n");
        assert_eq!(
            parse_photo(&raw),
            Some(Photo::Inline {
                media_type: "image/gif".into(),
                bytes: vec![1, 2, 3],
            })
        );
    }

    #[test]
    fn parse_photo_folded_base64_is_unfolded() {
        // A base64 PHOTO split across folded continuation lines must rejoin.
        let raw = "BEGIN:VCARD\r\nUID:u\r\nPHOTO;ENCODING=b;TYPE=JPEG:AQ\r\n ID\r\nEND:VCARD\r\n";
        assert_eq!(
            parse_photo(raw),
            Some(Photo::Inline {
                media_type: "image/jpeg".into(),
                bytes: vec![1, 2, 3],
            })
        );
    }

    #[test]
    fn parse_photo_inline_without_type_defaults_octet_stream() {
        let raw = format!("BEGIN:VCARD\r\nUID:u\r\nPHOTO;ENCODING=BASE64:{B64_3}\r\nEND:VCARD\r\n");
        let p = parse_photo(&raw).unwrap();
        assert!(
            matches!(p, Photo::Inline { media_type, .. } if media_type == "application/octet-stream")
        );
    }

    #[test]
    fn parse_photo_bad_base64_returns_none() {
        let raw =
            "BEGIN:VCARD\r\nUID:u\r\nPHOTO;ENCODING=b;TYPE=PNG:!!!notbase64!!!\r\nEND:VCARD\r\n";
        assert!(parse_photo(raw).is_none());
    }
}

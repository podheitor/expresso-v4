//! vCard (RFC 6350) emit helpers for the contact create/edit forms.
//!
//! The web tier builds a vCard from a [`ContactForm`] and ships it to the
//! contacts backend, which parses + indexes it on ingest. Kept in its own
//! module (alongside [`crate::ical`]) so the `\r\n`-heavy string-building stays
//! out of `routes.rs` — both to keep that file lean and because lizard's Rust
//! tokenizer miscounts the length of functions that emit many CRLF literals.

use serde::Deserialize;

#[derive(Deserialize)]
pub struct ContactForm {
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub given_name: String,
    #[serde(default)]
    pub family_name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub organization: String,
}

fn escape_vcard(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

/// Build a vCard 4.0 document from a contact form. `UID` is caller-supplied so
/// create (fresh UUID) and edit (existing UID) share one builder.
pub fn build_vcard(uid: &str, f: &ContactForm) -> String {
    let mut out = String::new();
    out.push_str("BEGIN:VCARD\r\n");
    out.push_str("VERSION:4.0\r\n");
    out.push_str(&format!("UID:{uid}\r\n"));
    // N: family;given;;; ;   FN: full_name (fallback to join)
    let family = escape_vcard(f.family_name.trim());
    let given = escape_vcard(f.given_name.trim());
    out.push_str(&format!("N:{family};{given};;;\r\n"));
    let fn_value = if f.full_name.trim().is_empty() {
        format!("{} {}", f.given_name.trim(), f.family_name.trim())
            .trim()
            .to_string()
    } else {
        f.full_name.trim().to_string()
    };
    if !fn_value.is_empty() {
        out.push_str(&format!("FN:{}\r\n", escape_vcard(&fn_value)));
    }
    if !f.email.trim().is_empty() {
        out.push_str(&format!(
            "EMAIL;TYPE=INTERNET:{}\r\n",
            escape_vcard(f.email.trim())
        ));
    }
    if !f.phone.trim().is_empty() {
        out.push_str(&format!(
            "TEL;TYPE=VOICE:{}\r\n",
            escape_vcard(f.phone.trim())
        ));
    }
    if !f.organization.trim().is_empty() {
        out.push_str(&format!("ORG:{}\r\n", escape_vcard(f.organization.trim())));
    }
    out.push_str("END:VCARD\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> ContactForm {
        ContactForm {
            full_name: String::new(),
            given_name: "Ana".into(),
            family_name: "Lima".into(),
            email: "ana@ex.com".into(),
            phone: "+55 11 9".into(),
            organization: "ACME".into(),
        }
    }

    #[test]
    fn build_vcard_emits_core_fields() {
        let v = build_vcard("uid-1", &form());
        assert!(v.starts_with("BEGIN:VCARD\r\nVERSION:4.0\r\n"));
        assert!(v.contains("UID:uid-1\r\n"));
        assert!(v.contains("N:Lima;Ana;;;\r\n"));
        // full_name empty → FN falls back to "given family"
        assert!(v.contains("FN:Ana Lima\r\n"));
        assert!(v.contains("EMAIL;TYPE=INTERNET:ana@ex.com\r\n"));
        assert!(v.contains("TEL;TYPE=VOICE:+55 11 9\r\n"));
        assert!(v.contains("ORG:ACME\r\n"));
        assert!(v.ends_with("END:VCARD\r\n"));
    }

    #[test]
    fn build_vcard_prefers_full_name_and_escapes() {
        let mut f = form();
        f.full_name = "Smith, John".into();
        let v = build_vcard("u", &f);
        // comma in FN is escaped, full_name wins over the join
        assert!(v.contains("FN:Smith\\, John\r\n"));
    }

    #[test]
    fn build_vcard_omits_blank_optional_fields() {
        let f = ContactForm {
            full_name: "Solo".into(),
            given_name: String::new(),
            family_name: String::new(),
            email: String::new(),
            phone: String::new(),
            organization: String::new(),
        };
        let v = build_vcard("u", &f);
        assert!(v.contains("FN:Solo\r\n"));
        assert!(!v.contains("EMAIL"));
        assert!(!v.contains("TEL"));
        assert!(!v.contains("ORG:"));
    }
}

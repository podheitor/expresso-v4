//! Email signatures — per-user HTML/plain signatures.
//!
//! GET    /api/v1/mail/signatures          — list signatures
//! POST   /api/v1/mail/signatures          — create signature
//! PUT    /api/v1/mail/signatures/:id      — update signature
//! DELETE /api/v1/mail/signatures/:id      — delete signature

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_SIGNATURE_BYTES: usize = 32 * 1024;
pub const MAX_SIGNATURE_NAME_BYTES: usize = 128;
pub const MAX_SIGNATURES_PER_USER: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SignatureFormat {
    Html,
    Plain,
}

impl SignatureFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Html  => "html",
            Self::Plain => "plain",
        }
    }
}

impl std::fmt::Display for SignatureFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    pub id:         Uuid,
    pub user_id:    Uuid,
    pub tenant_id:  Uuid,
    pub name:       String,
    pub content:    String,
    pub format:     SignatureFormat,
    pub is_default: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateSignatureBody {
    pub name:       String,
    pub content:    String,
    pub format:     Option<SignatureFormat>,
    pub is_default: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_signature_bytes_constant() {
        assert_eq!(MAX_SIGNATURE_BYTES, 32 * 1024);
    }

    #[test]
    fn max_signature_name_bytes_constant() {
        assert_eq!(MAX_SIGNATURE_NAME_BYTES, 128);
    }

    #[test]
    fn max_signatures_per_user_constant() {
        assert_eq!(MAX_SIGNATURES_PER_USER, 10);
    }

    #[test]
    fn format_html_as_str() {
        assert_eq!(SignatureFormat::Html.as_str(), "html");
    }

    #[test]
    fn format_plain_as_str() {
        assert_eq!(SignatureFormat::Plain.as_str(), "plain");
    }

    #[test]
    fn format_html_display() {
        assert_eq!(format!("{}", SignatureFormat::Html), "html");
    }

    #[test]
    fn format_plain_display() {
        assert_eq!(format!("{}", SignatureFormat::Plain), "plain");
    }

    #[test]
    fn format_equality() {
        assert_eq!(SignatureFormat::Html, SignatureFormat::Html);
        assert_ne!(SignatureFormat::Html, SignatureFormat::Plain);
    }

    #[test]
    fn format_serde_roundtrip_html() {
        let s = serde_json::to_string(&SignatureFormat::Html).unwrap();
        let back: SignatureFormat = serde_json::from_str(&s).unwrap();
        assert_eq!(back, SignatureFormat::Html);
    }

    #[test]
    fn format_serde_roundtrip_plain() {
        let s = serde_json::to_string(&SignatureFormat::Plain).unwrap();
        let back: SignatureFormat = serde_json::from_str(&s).unwrap();
        assert_eq!(back, SignatureFormat::Plain);
    }

    #[test]
    fn signature_serde_roundtrip() {
        let sig = Signature {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "Work".into(), content: "<p>Best</p>".into(),
            format: SignatureFormat::Html, is_default: true,
        };
        let s = serde_json::to_string(&sig).unwrap();
        let back: Signature = serde_json::from_str(&s).unwrap();
        assert_eq!(back.name, "Work");
        assert!(back.is_default);
    }

    #[test]
    fn create_signature_body_format_optional() {
        let b: CreateSignatureBody =
            serde_json::from_str(r#"{"name":"Sig","content":"Text"}"#).unwrap();
        assert!(b.format.is_none());
    }

    #[test]
    fn create_signature_body_is_default_optional() {
        let b: CreateSignatureBody =
            serde_json::from_str(r#"{"name":"Sig","content":"Text"}"#).unwrap();
        assert!(b.is_default.is_none());
    }

    #[test]
    fn create_signature_body_with_format_html() {
        let b: CreateSignatureBody =
            serde_json::from_str(r#"{"name":"S","content":"C","format":"html"}"#).unwrap();
        assert_eq!(b.format, Some(SignatureFormat::Html));
    }

    #[test]
    fn signature_clone_preserves_name() {
        let sig = Signature {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "Clone me".into(), content: "...".into(),
            format: SignatureFormat::Plain, is_default: false,
        };
        assert_eq!(sig.clone().name, "Clone me");
    }

    #[test]
    fn signature_is_default_false_field() {
        let sig = Signature {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "S".into(), content: "C".into(),
            format: SignatureFormat::Html, is_default: false,
        };
        assert!(!sig.is_default);
    }

    #[test]
    fn max_signatures_per_user_greater_than_zero() {
        assert!(MAX_SIGNATURES_PER_USER > 0);
    }

    #[test]
    fn max_signature_bytes_is_32_kib() {
        assert_eq!(MAX_SIGNATURE_BYTES, 32768);
    }

    #[test]
    fn signature_serde_json_contains_format_key() {
        let sig = Signature {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "S".into(), content: "C".into(),
            format: SignatureFormat::Html, is_default: false,
        };
        let s = serde_json::to_string(&sig).unwrap();
        assert!(s.contains("format"));
    }

    #[test]
    fn signature_serde_json_contains_is_default_key() {
        let sig = Signature {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "S".into(), content: "C".into(),
            format: SignatureFormat::Plain, is_default: true,
        };
        let s = serde_json::to_string(&sig).unwrap();
        assert!(s.contains("is_default"));
    }

    #[test]
    fn signature_user_id_accessible() {
        let uid = Uuid::new_v4();
        let sig = Signature {
            id: Uuid::nil(), user_id: uid, tenant_id: Uuid::nil(),
            name: "S".into(), content: "C".into(),
            format: SignatureFormat::Html, is_default: false,
        };
        assert_eq!(sig.user_id, uid);
    }

    #[test]
    fn create_body_is_default_true() {
        let b: CreateSignatureBody =
            serde_json::from_str(r#"{"name":"S","content":"C","is_default":true}"#).unwrap();
        assert_eq!(b.is_default, Some(true));
    }

    #[test]
    fn format_clone_preserves_variant() {
        let f = SignatureFormat::Html;
        assert_eq!(f.clone(), SignatureFormat::Html);
    }

    #[test]
    fn create_body_name_preserved() {
        let b: CreateSignatureBody =
            serde_json::from_str(r#"{"name":"My Sig","content":"X"}"#).unwrap();
        assert_eq!(b.name, "My Sig");
    }

    #[test]
    fn max_signature_name_bytes_positive() {
        assert!(MAX_SIGNATURE_NAME_BYTES > 0);
    }

    #[test]
    fn signature_serde_json_contains_content_key() {
        let sig = Signature {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "S".into(), content: "my content".into(),
            format: SignatureFormat::Plain, is_default: false,
        };
        let s = serde_json::to_string(&sig).unwrap();
        assert!(s.contains("content"));
    }
}

//! Admin domain management — add/remove/verify tenant domains.
//!
//! GET    /api/admin/domains             — list domains
//! POST   /api/admin/domains             — add domain
//! DELETE /api/admin/domains/:domain     — remove domain
//! POST   /api/admin/domains/:domain/verify — trigger DNS verification

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_DOMAIN_BYTES: usize = 253;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DomainStatus {
    Pending,
    Verified,
    Failed,
}

impl DomainStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending  => "pending",
            Self::Verified => "verified",
            Self::Failed   => "failed",
        }
    }
}

impl std::fmt::Display for DomainStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Domain {
    pub id:        Uuid,
    pub tenant_id: Uuid,
    pub name:      String,
    pub status:    DomainStatus,
    pub mx_record: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddDomainBody {
    pub name: String,
}

pub fn is_valid_domain(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_DOMAIN_BYTES
        && name.contains('.')
        && !name.starts_with('.')
        && !name.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_domain_bytes_constant() {
        assert_eq!(MAX_DOMAIN_BYTES, 253);
    }

    #[test]
    fn status_pending_as_str() {
        assert_eq!(DomainStatus::Pending.as_str(), "pending");
    }

    #[test]
    fn status_verified_as_str() {
        assert_eq!(DomainStatus::Verified.as_str(), "verified");
    }

    #[test]
    fn status_failed_as_str() {
        assert_eq!(DomainStatus::Failed.as_str(), "failed");
    }

    #[test]
    fn status_display_pending() {
        assert_eq!(format!("{}", DomainStatus::Pending), "pending");
    }

    #[test]
    fn status_display_verified() {
        assert_eq!(format!("{}", DomainStatus::Verified), "verified");
    }

    #[test]
    fn status_equality() {
        assert_eq!(DomainStatus::Verified, DomainStatus::Verified);
        assert_ne!(DomainStatus::Verified, DomainStatus::Failed);
    }

    #[test]
    fn status_serde_roundtrip_verified() {
        let s = serde_json::to_string(&DomainStatus::Verified).unwrap();
        let back: DomainStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, DomainStatus::Verified);
    }

    #[test]
    fn status_serde_roundtrip_pending() {
        let s = serde_json::to_string(&DomainStatus::Pending).unwrap();
        let back: DomainStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, DomainStatus::Pending);
    }

    #[test]
    fn valid_domain_simple() {
        assert!(is_valid_domain("example.com"));
    }

    #[test]
    fn invalid_domain_empty() {
        assert!(!is_valid_domain(""));
    }

    #[test]
    fn invalid_domain_no_dot() {
        assert!(!is_valid_domain("localhost"));
    }

    #[test]
    fn invalid_domain_starts_with_dot() {
        assert!(!is_valid_domain(".example.com"));
    }

    #[test]
    fn invalid_domain_ends_with_dot() {
        assert!(!is_valid_domain("example.com."));
    }

    #[test]
    fn valid_domain_subdomain() {
        assert!(is_valid_domain("mail.example.com"));
    }

    #[test]
    fn domain_serde_roundtrip() {
        let d = Domain {
            id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "example.com".into(), status: DomainStatus::Verified,
            mx_record: Some("10 mail.example.com".into()),
        };
        let s = serde_json::to_string(&d).unwrap();
        let back: Domain = serde_json::from_str(&s).unwrap();
        assert_eq!(back.name, "example.com");
        assert_eq!(back.status, DomainStatus::Verified);
    }

    #[test]
    fn domain_mx_record_optional() {
        let d = Domain {
            id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "x.com".into(), status: DomainStatus::Pending,
            mx_record: None,
        };
        assert!(d.mx_record.is_none());
    }

    #[test]
    fn add_domain_body_name_preserved() {
        let b: AddDomainBody =
            serde_json::from_str(r#"{"name":"newdomain.com"}"#).unwrap();
        assert_eq!(b.name, "newdomain.com");
    }

    #[test]
    fn status_copy_trait() {
        let s = DomainStatus::Failed;
        let s2 = s;
        assert_eq!(s, s2);
    }

    #[test]
    fn domain_clone_preserves_name() {
        let d = Domain {
            id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "clone.com".into(), status: DomainStatus::Pending,
            mx_record: None,
        };
        assert_eq!(d.clone().name, "clone.com");
    }

    #[test]
    fn status_debug_contains_variant() {
        let s = format!("{:?}", DomainStatus::Failed);
        assert!(s.contains("Failed"));
    }

    #[test]
    fn domain_json_contains_status_key() {
        let d = Domain {
            id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "d.com".into(), status: DomainStatus::Verified, mx_record: None,
        };
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains("status"));
    }

    #[test]
    fn max_domain_bytes_is_rfc1035_limit() {
        assert_eq!(MAX_DOMAIN_BYTES, 253);
    }

    #[test]
    fn valid_domain_with_hyphen() {
        assert!(is_valid_domain("my-company.com"));
    }

    #[test]
    fn status_serde_roundtrip_failed() {
        let s = serde_json::to_string(&DomainStatus::Failed).unwrap();
        let back: DomainStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, DomainStatus::Failed);
    }

    #[test]
    fn valid_domain_deep_subdomain() {
        assert!(is_valid_domain("a.b.c.example.com"));
    }
}

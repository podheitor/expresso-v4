//! Mail address aliases — per-user delivery aliases.
//!
//! GET    /api/v1/mail/aliases          — list aliases for caller
//! POST   /api/v1/mail/aliases          — create alias
//! DELETE /api/v1/mail/aliases/:alias   — remove alias

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_ALIAS_BYTES: usize = 254;
pub const MAX_ALIASES_PER_USER: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alias {
    pub id:        Uuid,
    pub user_id:   Uuid,
    pub tenant_id: Uuid,
    pub address:   String,
    pub enabled:   bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateAliasBody {
    pub address: String,
    pub enabled: Option<bool>,
}

pub fn is_valid_alias(address: &str) -> bool {
    !address.is_empty()
        && address.len() <= MAX_ALIAS_BYTES
        && address.contains('@')
        && !address.starts_with('@')
        && !address.ends_with('@')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_alias_bytes_constant() {
        assert_eq!(MAX_ALIAS_BYTES, 254);
    }

    #[test]
    fn max_aliases_per_user_constant() {
        assert_eq!(MAX_ALIASES_PER_USER, 20);
    }

    #[test]
    fn valid_alias_simple() {
        assert!(is_valid_alias("alias@example.com"));
    }

    #[test]
    fn invalid_alias_empty() {
        assert!(!is_valid_alias(""));
    }

    #[test]
    fn invalid_alias_no_at_sign() {
        assert!(!is_valid_alias("noatsign"));
    }

    #[test]
    fn invalid_alias_starts_with_at() {
        assert!(!is_valid_alias("@example.com"));
    }

    #[test]
    fn invalid_alias_ends_with_at() {
        assert!(!is_valid_alias("user@"));
    }

    #[test]
    fn alias_serde_roundtrip() {
        let a = Alias {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            address: "alias@example.com".into(), enabled: true,
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: Alias = serde_json::from_str(&s).unwrap();
        assert_eq!(back.address, "alias@example.com");
        assert!(back.enabled);
    }

    #[test]
    fn create_alias_body_enabled_optional() {
        let b: CreateAliasBody = serde_json::from_str(r#"{"address":"a@b.com"}"#).unwrap();
        assert!(b.enabled.is_none());
    }

    #[test]
    fn create_alias_body_enabled_true() {
        let b: CreateAliasBody =
            serde_json::from_str(r#"{"address":"a@b.com","enabled":true}"#).unwrap();
        assert_eq!(b.enabled, Some(true));
    }

    #[test]
    fn alias_clone_preserves_address() {
        let a = Alias {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            address: "clone@test.com".into(), enabled: false,
        };
        assert_eq!(a.clone().address, "clone@test.com");
    }

    #[test]
    fn valid_alias_subdomain() {
        assert!(is_valid_alias("user@sub.domain.com"));
    }

    #[test]
    fn alias_enabled_field_preserved() {
        let a = Alias {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            address: "x@y.com".into(), enabled: false,
        };
        assert!(!a.enabled);
    }

    #[test]
    fn alias_address_field_accessible() {
        let a = Alias {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            address: "test@domain.com".into(), enabled: true,
        };
        assert!(a.address.contains('@'));
    }

    #[test]
    fn create_alias_body_address_preserved() {
        let b: CreateAliasBody =
            serde_json::from_str(r#"{"address":"hello@world.com"}"#).unwrap();
        assert_eq!(b.address, "hello@world.com");
    }

    #[test]
    fn invalid_alias_too_long() {
        let long = format!("{}@example.com", "a".repeat(250));
        assert!(!is_valid_alias(&long));
    }

    #[test]
    fn max_alias_bytes_is_rfc5321_limit() {
        assert_eq!(MAX_ALIAS_BYTES, 254);
    }

    #[test]
    fn alias_user_id_accessible() {
        let uid = Uuid::new_v4();
        let a = Alias {
            id: Uuid::nil(), user_id: uid, tenant_id: Uuid::nil(),
            address: "a@b.com".into(), enabled: true,
        };
        assert_eq!(a.user_id, uid);
    }

    #[test]
    fn create_alias_body_enabled_false() {
        let b: CreateAliasBody =
            serde_json::from_str(r#"{"address":"x@y.com","enabled":false}"#).unwrap();
        assert_eq!(b.enabled, Some(false));
    }

    #[test]
    fn valid_alias_plus_addressing() {
        assert!(is_valid_alias("user+tag@example.com"));
    }

    #[test]
    fn max_aliases_per_user_greater_than_zero() {
        assert!(MAX_ALIASES_PER_USER > 0);
    }

    #[test]
    fn alias_serde_json_contains_address_key() {
        let a = Alias {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            address: "x@y.com".into(), enabled: true,
        };
        let s = serde_json::to_string(&a).unwrap();
        assert!(s.contains("address"));
    }

    #[test]
    fn alias_serde_json_contains_enabled_key() {
        let a = Alias {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            address: "x@y.com".into(), enabled: true,
        };
        let s = serde_json::to_string(&a).unwrap();
        assert!(s.contains("enabled"));
    }

    #[test]
    fn alias_tenant_id_accessible() {
        let tid = Uuid::new_v4();
        let a = Alias {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: tid,
            address: "a@b.com".into(), enabled: true,
        };
        assert_eq!(a.tenant_id, tid);
    }
}

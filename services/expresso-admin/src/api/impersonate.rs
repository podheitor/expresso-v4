//! Admin impersonation — generate a short-lived token for a target user.
//!
//! POST /api/admin/impersonate — SuperAdmin only. Returns a short-lived JWT.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const IMPERSONATE_TTL_SECS: u64 = 900;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpersonateRequest {
    pub target_user_id: Uuid,
    pub tenant_id:      Uuid,
    pub reason:         Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpersonateResponse {
    pub token:      String,
    pub expires_in: u64,
    pub user_id:    Uuid,
    pub tenant_id:  Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImpersonateScope {
    ReadOnly,
    Full,
}

impl ImpersonateScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Full     => "full",
        }
    }
}

impl std::fmt::Display for ImpersonateScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_constant_value() {
        assert_eq!(IMPERSONATE_TTL_SECS, 900);
    }

    #[test]
    fn ttl_is_fifteen_minutes() {
        assert_eq!(IMPERSONATE_TTL_SECS, 15 * 60);
    }

    #[test]
    fn scope_read_only_as_str() {
        assert_eq!(ImpersonateScope::ReadOnly.as_str(), "read_only");
    }

    #[test]
    fn scope_full_as_str() {
        assert_eq!(ImpersonateScope::Full.as_str(), "full");
    }

    #[test]
    fn scope_display_read_only() {
        assert_eq!(format!("{}", ImpersonateScope::ReadOnly), "read_only");
    }

    #[test]
    fn scope_display_full() {
        assert_eq!(format!("{}", ImpersonateScope::Full), "full");
    }

    #[test]
    fn scope_equality() {
        assert_eq!(ImpersonateScope::Full, ImpersonateScope::Full);
        assert_ne!(ImpersonateScope::Full, ImpersonateScope::ReadOnly);
    }

    #[test]
    fn scope_serde_roundtrip_full() {
        let s = serde_json::to_string(&ImpersonateScope::Full).unwrap();
        let back: ImpersonateScope = serde_json::from_str(&s).unwrap();
        assert_eq!(back, ImpersonateScope::Full);
    }

    #[test]
    fn scope_serde_roundtrip_read_only() {
        let s = serde_json::to_string(&ImpersonateScope::ReadOnly).unwrap();
        let back: ImpersonateScope = serde_json::from_str(&s).unwrap();
        assert_eq!(back, ImpersonateScope::ReadOnly);
    }

    #[test]
    fn request_serde_roundtrip() {
        let r = ImpersonateRequest {
            target_user_id: Uuid::nil(),
            tenant_id:      Uuid::nil(),
            reason:         Some("support ticket #123".into()),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: ImpersonateRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.reason.as_deref(), Some("support ticket #123"));
    }

    #[test]
    fn request_reason_optional() {
        let r: ImpersonateRequest = serde_json::from_str(
            &format!(r#"{{"target_user_id":"{0}","tenant_id":"{0}"}}"#, Uuid::nil()),
        ).unwrap();
        assert!(r.reason.is_none());
    }

    #[test]
    fn response_serde_roundtrip() {
        let r = ImpersonateResponse {
            token:      "eyJhbGc...".into(),
            expires_in: IMPERSONATE_TTL_SECS,
            user_id:    Uuid::nil(),
            tenant_id:  Uuid::nil(),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: ImpersonateResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.expires_in, IMPERSONATE_TTL_SECS);
    }

    #[test]
    fn response_token_preserved() {
        let r = ImpersonateResponse {
            token:      "my-token".into(),
            expires_in: 900,
            user_id:    Uuid::nil(),
            tenant_id:  Uuid::nil(),
        };
        assert_eq!(r.token, "my-token");
    }

    #[test]
    fn request_clone_preserves_reason() {
        let r = ImpersonateRequest {
            target_user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            reason: Some("audit".into()),
        };
        assert_eq!(r.clone().reason.as_deref(), Some("audit"));
    }

    #[test]
    fn scope_copy_trait() {
        let s = ImpersonateScope::ReadOnly;
        let s2 = s;
        assert_eq!(s, s2);
    }

    #[test]
    fn response_expires_in_matches_ttl_constant() {
        let r = ImpersonateResponse {
            token: "t".into(), expires_in: IMPERSONATE_TTL_SECS,
            user_id: Uuid::nil(), tenant_id: Uuid::nil(),
        };
        assert_eq!(r.expires_in, 900);
    }

    #[test]
    fn scope_debug_contains_variant() {
        let s = format!("{:?}", ImpersonateScope::Full);
        assert!(s.contains("Full"));
    }

    #[test]
    fn response_json_contains_token_key() {
        let r = ImpersonateResponse {
            token: "tok".into(), expires_in: 900,
            user_id: Uuid::nil(), tenant_id: Uuid::nil(),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("token"));
    }

    #[test]
    fn request_json_contains_target_user_id_key() {
        let r = ImpersonateRequest {
            target_user_id: Uuid::nil(), tenant_id: Uuid::nil(), reason: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("target_user_id"));
    }

    #[test]
    fn ttl_is_positive() {
        assert!(IMPERSONATE_TTL_SECS > 0);
    }

    #[test]
    fn scope_read_only_serde_roundtrip() {
        let s = serde_json::to_string(&ImpersonateScope::ReadOnly).unwrap();
        let back: ImpersonateScope = serde_json::from_str(&s).unwrap();
        assert_eq!(back, ImpersonateScope::ReadOnly);
    }

    #[test]
    fn request_target_user_id_preserved() {
        let uid = Uuid::new_v4();
        let r = ImpersonateRequest {
            target_user_id: uid, tenant_id: Uuid::nil(), reason: None,
        };
        assert_eq!(r.target_user_id, uid);
    }

    #[test]
    fn response_user_id_preserved() {
        let uid = Uuid::new_v4();
        let r = ImpersonateResponse {
            token: "t".into(), expires_in: 900,
            user_id: uid, tenant_id: Uuid::nil(),
        };
        assert_eq!(r.user_id, uid);
    }

    #[test]
    fn scope_debug_contains_read_only() {
        let s = format!("{:?}", ImpersonateScope::ReadOnly);
        assert!(s.contains("ReadOnly"));
    }

    #[test]
    fn response_tenant_id_preserved() {
        let tid = uuid::Uuid::new_v4();
        let r = ImpersonateResponse {
            token: "t".into(), expires_in: 900,
            user_id: uuid::Uuid::nil(), tenant_id: tid,
        };
        assert_eq!(r.tenant_id, tid);
    }

    #[test]
    fn scope_full_and_read_only_have_distinct_str() {
        assert_ne!(ImpersonateScope::Full.as_str(), ImpersonateScope::ReadOnly.as_str());
    }
}

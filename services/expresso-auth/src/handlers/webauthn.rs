//! WebAuthn (FIDO2 / Passkey) handlers.
//!
//! POST /auth/webauthn/register/begin    — begin registration ceremony
//! POST /auth/webauthn/register/finish   — finish registration ceremony
//! POST /auth/webauthn/login/begin       — begin authentication ceremony
//! POST /auth/webauthn/login/finish      — finish authentication ceremony

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_CREDENTIAL_NAME_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticatorAttachment {
    Platform,
    CrossPlatform,
}

impl AuthenticatorAttachment {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Platform      => "platform",
            Self::CrossPlatform => "cross-platform",
        }
    }
}

impl std::fmt::Display for AuthenticatorAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebAuthnCredential {
    pub id:           Uuid,
    pub user_id:      Uuid,
    pub credential_id: String,
    pub name:         String,
    pub attachment:   Option<AuthenticatorAttachment>,
    pub sign_count:   u32,
}

#[derive(Debug, Deserialize)]
pub struct RegisterBeginBody {
    pub credential_name:    Option<String>,
    pub attachment:         Option<AuthenticatorAttachment>,
}

#[derive(Debug, Deserialize)]
pub struct LoginBeginBody {
    pub user_id: Option<Uuid>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_credential_name_bytes_constant() {
        assert_eq!(MAX_CREDENTIAL_NAME_BYTES, 64);
    }

    #[test]
    fn attachment_platform_as_str() {
        assert_eq!(AuthenticatorAttachment::Platform.as_str(), "platform");
    }

    #[test]
    fn attachment_cross_platform_as_str() {
        assert_eq!(AuthenticatorAttachment::CrossPlatform.as_str(), "cross-platform");
    }

    #[test]
    fn attachment_display_platform() {
        assert_eq!(format!("{}", AuthenticatorAttachment::Platform), "platform");
    }

    #[test]
    fn attachment_display_cross_platform() {
        assert_eq!(format!("{}", AuthenticatorAttachment::CrossPlatform), "cross-platform");
    }

    #[test]
    fn attachment_equality() {
        assert_eq!(AuthenticatorAttachment::Platform, AuthenticatorAttachment::Platform);
        assert_ne!(AuthenticatorAttachment::Platform, AuthenticatorAttachment::CrossPlatform);
    }

    #[test]
    fn attachment_serde_roundtrip_platform() {
        let s = serde_json::to_string(&AuthenticatorAttachment::Platform).unwrap();
        let back: AuthenticatorAttachment = serde_json::from_str(&s).unwrap();
        assert_eq!(back, AuthenticatorAttachment::Platform);
    }

    #[test]
    fn attachment_serde_roundtrip_cross_platform() {
        let s = serde_json::to_string(&AuthenticatorAttachment::CrossPlatform).unwrap();
        let back: AuthenticatorAttachment = serde_json::from_str(&s).unwrap();
        assert_eq!(back, AuthenticatorAttachment::CrossPlatform);
    }

    #[test]
    fn credential_serde_roundtrip() {
        let c = WebAuthnCredential {
            id: Uuid::nil(), user_id: Uuid::nil(),
            credential_id: "cred-id-abc".into(),
            name: "My Yubikey".into(),
            attachment: Some(AuthenticatorAttachment::CrossPlatform),
            sign_count: 7,
        };
        let s = serde_json::to_string(&c).unwrap();
        let back: WebAuthnCredential = serde_json::from_str(&s).unwrap();
        assert_eq!(back.name, "My Yubikey");
        assert_eq!(back.sign_count, 7);
    }

    #[test]
    fn register_begin_body_all_optional() {
        let b: RegisterBeginBody = serde_json::from_str(r#"{}"#).unwrap();
        assert!(b.credential_name.is_none());
        assert!(b.attachment.is_none());
    }

    #[test]
    fn register_begin_body_with_name() {
        let b: RegisterBeginBody =
            serde_json::from_str(r#"{"credential_name":"My Key"}"#).unwrap();
        assert_eq!(b.credential_name.as_deref(), Some("My Key"));
    }

    #[test]
    fn login_begin_body_user_id_optional() {
        let b: LoginBeginBody = serde_json::from_str(r#"{}"#).unwrap();
        assert!(b.user_id.is_none());
    }

    #[test]
    fn credential_clone_preserves_name() {
        let c = WebAuthnCredential {
            id: Uuid::nil(), user_id: Uuid::nil(),
            credential_id: "x".into(), name: "Passkey".into(),
            attachment: None, sign_count: 0,
        };
        assert_eq!(c.clone().name, "Passkey");
    }

    #[test]
    fn attachment_copy_trait() {
        let a = AuthenticatorAttachment::Platform;
        let a2 = a;
        assert_eq!(a, a2);
    }

    #[test]
    fn credential_sign_count_preserved() {
        let c = WebAuthnCredential {
            id: Uuid::nil(), user_id: Uuid::nil(),
            credential_id: "id".into(), name: "N".into(),
            attachment: None, sign_count: 42,
        };
        assert_eq!(c.sign_count, 42);
    }

    #[test]
    fn attachment_debug_contains_variant() {
        let s = format!("{:?}", AuthenticatorAttachment::CrossPlatform);
        assert!(s.contains("CrossPlatform"));
    }

    #[test]
    fn credential_json_contains_sign_count_key() {
        let c = WebAuthnCredential {
            id: Uuid::nil(), user_id: Uuid::nil(),
            credential_id: "id".into(), name: "N".into(),
            attachment: None, sign_count: 1,
        };
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("sign_count"));
    }

    #[test]
    fn max_credential_name_bytes_positive() {
        assert!(MAX_CREDENTIAL_NAME_BYTES > 0);
    }

    #[test]
    fn register_begin_attachment_platform() {
        let b: RegisterBeginBody =
            serde_json::from_str(r#"{"attachment":"platform"}"#).unwrap();
        assert_eq!(b.attachment, Some(AuthenticatorAttachment::Platform));
    }

    #[test]
    fn credential_attachment_none_when_absent() {
        let c = WebAuthnCredential {
            id: Uuid::nil(), user_id: Uuid::nil(),
            credential_id: "id".into(), name: "N".into(),
            attachment: None, sign_count: 0,
        };
        assert!(c.attachment.is_none());
    }

    #[test]
    fn attachment_display_cross_platform_hyphen() {
        assert!(AuthenticatorAttachment::CrossPlatform.as_str().contains('-'));
    }

    #[test]
    fn credential_id_field_accessible() {
        let c = WebAuthnCredential {
            id: Uuid::nil(), user_id: Uuid::nil(),
            credential_id: "unique-cred-id".into(), name: "Key".into(),
            attachment: None, sign_count: 0,
        };
        assert_eq!(c.credential_id, "unique-cred-id");
    }

    #[test]
    fn attachment_serde_roundtrip_cross_platform_json() {
        let s = serde_json::to_string(&AuthenticatorAttachment::CrossPlatform).unwrap();
        assert!(s.contains("cross_platform"));
    }

    #[test]
    fn max_credential_name_bytes_is_64() {
        assert_eq!(MAX_CREDENTIAL_NAME_BYTES, 64);
    }

    #[test]
    fn both_attachment_variants_have_distinct_str() {
        assert_ne!(
            AuthenticatorAttachment::Platform.as_str(),
            AuthenticatorAttachment::CrossPlatform.as_str(),
        );
    }

    #[test]
    fn credential_user_id_accessible() {
        let uid = Uuid::new_v4();
        let c = WebAuthnCredential {
            id: Uuid::nil(), user_id: uid,
            credential_id: "cid".into(), name: "Key".into(),
            attachment: None, sign_count: 0,
        };
        assert_eq!(c.user_id, uid);
    }
}

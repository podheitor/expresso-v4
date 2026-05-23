//! Multi-factor authentication (TOTP) handlers.
//!
//! POST /auth/mfa/setup    — generate TOTP secret + QR URI
//! POST /auth/mfa/verify   — verify a TOTP code and enable MFA
//! DELETE /auth/mfa        — disable MFA for the caller

use serde::{Deserialize, Serialize};

pub const TOTP_DIGITS:  u32 = 6;
pub const TOTP_PERIOD:  u32 = 30;
pub const TOTP_ISSUER:  &str = "Expresso";
pub const TOTP_ALGO:    &str = "SHA1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MfaStatus {
    Disabled,
    Pending,
    Enabled,
}

impl MfaStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Pending  => "pending",
            Self::Enabled  => "enabled",
        }
    }
}

impl std::fmt::Display for MfaStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MfaSetupResponse {
    pub secret:      String,
    pub otpauth_uri: String,
    pub status:      MfaStatus,
}

#[derive(Debug, Deserialize)]
pub struct MfaVerifyBody {
    pub code:   String,
    pub secret: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_digits_constant() {
        assert_eq!(TOTP_DIGITS, 6);
    }

    #[test]
    fn totp_period_constant() {
        assert_eq!(TOTP_PERIOD, 30);
    }

    #[test]
    fn totp_issuer_constant() {
        assert_eq!(TOTP_ISSUER, "Expresso");
    }

    #[test]
    fn totp_algo_constant() {
        assert_eq!(TOTP_ALGO, "SHA1");
    }

    #[test]
    fn status_disabled_as_str() {
        assert_eq!(MfaStatus::Disabled.as_str(), "disabled");
    }

    #[test]
    fn status_pending_as_str() {
        assert_eq!(MfaStatus::Pending.as_str(), "pending");
    }

    #[test]
    fn status_enabled_as_str() {
        assert_eq!(MfaStatus::Enabled.as_str(), "enabled");
    }

    #[test]
    fn status_display_enabled() {
        assert_eq!(format!("{}", MfaStatus::Enabled), "enabled");
    }

    #[test]
    fn status_equality() {
        assert_eq!(MfaStatus::Enabled, MfaStatus::Enabled);
        assert_ne!(MfaStatus::Enabled, MfaStatus::Disabled);
    }

    #[test]
    fn status_serde_roundtrip_enabled() {
        let s = serde_json::to_string(&MfaStatus::Enabled).unwrap();
        let back: MfaStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, MfaStatus::Enabled);
    }

    #[test]
    fn status_serde_roundtrip_pending() {
        let s = serde_json::to_string(&MfaStatus::Pending).unwrap();
        let back: MfaStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, MfaStatus::Pending);
    }

    #[test]
    fn setup_response_serde_roundtrip() {
        let r = MfaSetupResponse {
            secret: "BASE32SECRET".into(),
            otpauth_uri: "otpauth://totp/Expresso:user?secret=BASE32SECRET".into(),
            status: MfaStatus::Pending,
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: MfaSetupResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.secret, "BASE32SECRET");
        assert_eq!(back.status, MfaStatus::Pending);
    }

    #[test]
    fn verify_body_secret_optional() {
        let b: MfaVerifyBody = serde_json::from_str(r#"{"code":"123456"}"#).unwrap();
        assert!(b.secret.is_none());
        assert_eq!(b.code, "123456");
    }

    #[test]
    fn verify_body_with_secret() {
        let b: MfaVerifyBody =
            serde_json::from_str(r#"{"code":"654321","secret":"ABC"}"#).unwrap();
        assert_eq!(b.secret.as_deref(), Some("ABC"));
    }

    #[test]
    fn setup_response_clone_preserves_secret() {
        let r = MfaSetupResponse {
            secret: "MYSECRET".into(), otpauth_uri: "otpauth://x".into(),
            status: MfaStatus::Pending,
        };
        assert_eq!(r.clone().secret, "MYSECRET");
    }

    #[test]
    fn status_copy_trait() {
        let s = MfaStatus::Disabled;
        let s2 = s;
        assert_eq!(s, s2);
    }

    #[test]
    fn totp_digits_is_six() {
        assert_eq!(TOTP_DIGITS, 6);
    }

    #[test]
    fn totp_period_is_30_secs() {
        assert_eq!(TOTP_PERIOD, 30);
    }

    #[test]
    fn status_debug_contains_variant() {
        let s = format!("{:?}", MfaStatus::Enabled);
        assert!(s.contains("Enabled"));
    }

    #[test]
    fn setup_response_json_contains_status_key() {
        let r = MfaSetupResponse {
            secret: "S".into(), otpauth_uri: "O".into(), status: MfaStatus::Pending,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("status"));
    }

    #[test]
    fn setup_response_json_contains_secret_key() {
        let r = MfaSetupResponse {
            secret: "S".into(), otpauth_uri: "O".into(), status: MfaStatus::Pending,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("secret"));
    }

    #[test]
    fn totp_issuer_nonempty() {
        assert!(!TOTP_ISSUER.is_empty());
    }

    #[test]
    fn status_serde_roundtrip_disabled() {
        let s = serde_json::to_string(&MfaStatus::Disabled).unwrap();
        let back: MfaStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, MfaStatus::Disabled);
    }

    #[test]
    fn totp_algo_is_sha1() {
        assert_eq!(TOTP_ALGO, "SHA1");
    }
}

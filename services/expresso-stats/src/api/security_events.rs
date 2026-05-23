use serde::{Deserialize, Serialize};

pub const DEFAULT_DAYS: u32 = 7;
pub const MAX_DAYS: u32 = 90;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecurityEventKind {
    LoginFailure,
    LoginSuccess,
    AccountLocked,
    PasswordChanged,
    TwoFaEnabled,
    TwoFaDisabled,
    SuspiciousLogin,
    ApiKeyCreated,
    ApiKeyRevoked,
}

impl std::fmt::Display for SecurityEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecurityEventKind::LoginFailure     => write!(f, "login_failure"),
            SecurityEventKind::LoginSuccess     => write!(f, "login_success"),
            SecurityEventKind::AccountLocked    => write!(f, "account_locked"),
            SecurityEventKind::PasswordChanged  => write!(f, "password_changed"),
            SecurityEventKind::TwoFaEnabled     => write!(f, "2fa_enabled"),
            SecurityEventKind::TwoFaDisabled    => write!(f, "2fa_disabled"),
            SecurityEventKind::SuspiciousLogin  => write!(f, "suspicious_login"),
            SecurityEventKind::ApiKeyCreated    => write!(f, "api_key_created"),
            SecurityEventKind::ApiKeyRevoked    => write!(f, "api_key_revoked"),
        }
    }
}

impl SecurityEventKind {
    pub fn is_critical(&self) -> bool {
        matches!(
            self,
            SecurityEventKind::AccountLocked
                | SecurityEventKind::SuspiciousLogin
        )
    }

    pub fn is_auth_event(&self) -> bool {
        matches!(
            self,
            SecurityEventKind::LoginFailure
                | SecurityEventKind::LoginSuccess
                | SecurityEventKind::AccountLocked
                | SecurityEventKind::SuspiciousLogin
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityEventsParams {
    pub tenant_id:  String,
    #[serde(default = "default_days")]
    pub days:       u32,
    pub event_kind: Option<String>,
}

fn default_days() -> u32 {
    DEFAULT_DAYS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecurityEventRow {
    pub date:   String,
    pub kind:   String,
    pub count:  i64,
}

pub fn clamp_days(days: u32) -> u32 {
    days.max(1).min(MAX_DAYS)
}

pub fn failure_rate(failures: u64, attempts: u64) -> f64 {
    if attempts == 0 { 0.0 } else { failures as f64 / attempts as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_days_constant() {
        assert_eq!(DEFAULT_DAYS, 7);
    }

    #[test]
    fn max_days_constant() {
        assert_eq!(MAX_DAYS, 90);
    }

    #[test]
    fn event_display_login_failure() {
        assert_eq!(SecurityEventKind::LoginFailure.to_string(), "login_failure");
    }

    #[test]
    fn event_display_login_success() {
        assert_eq!(SecurityEventKind::LoginSuccess.to_string(), "login_success");
    }

    #[test]
    fn event_display_account_locked() {
        assert_eq!(SecurityEventKind::AccountLocked.to_string(), "account_locked");
    }

    #[test]
    fn event_display_password_changed() {
        assert_eq!(SecurityEventKind::PasswordChanged.to_string(), "password_changed");
    }

    #[test]
    fn event_display_2fa_enabled() {
        assert_eq!(SecurityEventKind::TwoFaEnabled.to_string(), "2fa_enabled");
    }

    #[test]
    fn event_display_2fa_disabled() {
        assert_eq!(SecurityEventKind::TwoFaDisabled.to_string(), "2fa_disabled");
    }

    #[test]
    fn event_display_suspicious_login() {
        assert_eq!(SecurityEventKind::SuspiciousLogin.to_string(), "suspicious_login");
    }

    #[test]
    fn event_display_api_key_created() {
        assert_eq!(SecurityEventKind::ApiKeyCreated.to_string(), "api_key_created");
    }

    #[test]
    fn event_display_api_key_revoked() {
        assert_eq!(SecurityEventKind::ApiKeyRevoked.to_string(), "api_key_revoked");
    }

    #[test]
    fn account_locked_is_critical() {
        assert!(SecurityEventKind::AccountLocked.is_critical());
    }

    #[test]
    fn suspicious_login_is_critical() {
        assert!(SecurityEventKind::SuspiciousLogin.is_critical());
    }

    #[test]
    fn login_failure_not_critical() {
        assert!(!SecurityEventKind::LoginFailure.is_critical());
    }

    #[test]
    fn login_failure_is_auth_event() {
        assert!(SecurityEventKind::LoginFailure.is_auth_event());
    }

    #[test]
    fn password_changed_not_auth_event() {
        assert!(!SecurityEventKind::PasswordChanged.is_auth_event());
    }

    #[test]
    fn failure_rate_zero_attempts() {
        assert_eq!(failure_rate(10, 0), 0.0);
    }

    #[test]
    fn failure_rate_half() {
        assert!((failure_rate(5, 10) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn failure_rate_all_failures() {
        assert!((failure_rate(10, 10) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn clamp_days_below_min() {
        assert_eq!(clamp_days(0), 1);
    }

    #[test]
    fn clamp_days_above_max() {
        assert_eq!(clamp_days(200), MAX_DAYS);
    }

    #[test]
    fn default_days_fn_returns_7() {
        assert_eq!(default_days(), 7);
    }

    #[test]
    fn event_equality() {
        assert_eq!(SecurityEventKind::LoginFailure, SecurityEventKind::LoginFailure);
        assert_ne!(SecurityEventKind::LoginFailure, SecurityEventKind::LoginSuccess);
    }

    #[test]
    fn serde_roundtrip_event_kind() {
        let k = SecurityEventKind::SuspiciousLogin;
        let json = serde_json::to_string(&k).unwrap();
        let back: SecurityEventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SecurityEventKind::SuspiciousLogin);
    }

    #[test]
    fn security_event_row_serde_roundtrip() {
        let row = SecurityEventRow { date: "2026-05-01".into(), kind: "login_failure".into(), count: 42 };
        let json = serde_json::to_string(&row).unwrap();
        let back: SecurityEventRow = serde_json::from_str(&json).unwrap();
        assert_eq!(back, row);
    }

    #[test]
    fn login_success_is_auth_event() {
        assert!(SecurityEventKind::LoginSuccess.is_auth_event());
    }
}

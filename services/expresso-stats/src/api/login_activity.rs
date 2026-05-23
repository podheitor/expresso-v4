//! Login activity analytics types and helpers — sprint #19857.

use serde::{Deserialize, Serialize};

/// Login event outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginOutcome {
    Success,
    Failure,
    Blocked,
}

impl std::fmt::Display for LoginOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginOutcome::Success => write!(f, "success"),
            LoginOutcome::Failure => write!(f, "failure"),
            LoginOutcome::Blocked => write!(f, "blocked"),
        }
    }
}

/// Aggregated login activity for a single time bucket + outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginActivityEntry {
    pub bucket:  String,
    pub outcome: LoginOutcome,
    pub count:   u64,
}

/// Request parameters for login activity endpoint.
#[derive(Debug, Deserialize)]
pub struct LoginActivityParams {
    pub tenant_id:  String,
    pub since_days: Option<u32>,
    pub user_id:    Option<String>,
}

pub const DEFAULT_SINCE_DAYS: u32 = 7;
pub const MAX_SINCE_DAYS:     u32 = 90;

pub fn validate_login_activity_params(p: &LoginActivityParams) -> Option<String> {
    if p.tenant_id.trim().is_empty() {
        return Some("tenant_id must not be empty".into());
    }
    if let Some(d) = p.since_days {
        if d == 0 {
            return Some("since_days must be >= 1".into());
        }
        if d > MAX_SINCE_DAYS {
            return Some(format!("since_days must be <= {}", MAX_SINCE_DAYS));
        }
    }
    None
}

/// Computes the failure rate from success/failure counts. Returns 0.0 when
/// total is 0.
pub fn failure_rate(failures: u64, total: u64) -> f64 {
    if total == 0 { return 0.0; }
    failures as f64 / total as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_outcome_success_display() {
        assert_eq!(LoginOutcome::Success.to_string(), "success");
    }

    #[test]
    fn login_outcome_failure_display() {
        assert_eq!(LoginOutcome::Failure.to_string(), "failure");
    }

    #[test]
    fn login_outcome_blocked_display() {
        assert_eq!(LoginOutcome::Blocked.to_string(), "blocked");
    }

    #[test]
    fn login_outcome_eq_same_variant() {
        assert_eq!(LoginOutcome::Success, LoginOutcome::Success);
    }

    #[test]
    fn login_outcome_ne_different_variants() {
        assert_ne!(LoginOutcome::Success, LoginOutcome::Failure);
    }

    #[test]
    fn default_since_days_is_7() {
        assert_eq!(DEFAULT_SINCE_DAYS, 7);
    }

    #[test]
    fn max_since_days_is_90() {
        assert_eq!(MAX_SINCE_DAYS, 90);
    }

    #[test]
    fn validate_empty_tenant_returns_error() {
        let p = LoginActivityParams { tenant_id: "".into(), since_days: None, user_id: None };
        assert!(validate_login_activity_params(&p).is_some());
    }

    #[test]
    fn validate_valid_params_returns_none() {
        let p = LoginActivityParams { tenant_id: "t1".into(), since_days: Some(7), user_id: None };
        assert!(validate_login_activity_params(&p).is_none());
    }

    #[test]
    fn validate_since_days_zero_returns_error() {
        let p = LoginActivityParams { tenant_id: "t1".into(), since_days: Some(0), user_id: None };
        assert!(validate_login_activity_params(&p).is_some());
    }

    #[test]
    fn validate_since_days_above_max_returns_error() {
        let p = LoginActivityParams { tenant_id: "t1".into(), since_days: Some(MAX_SINCE_DAYS + 1), user_id: None };
        assert!(validate_login_activity_params(&p).is_some());
    }

    #[test]
    fn validate_since_days_at_max_is_ok() {
        let p = LoginActivityParams { tenant_id: "t1".into(), since_days: Some(MAX_SINCE_DAYS), user_id: None };
        assert!(validate_login_activity_params(&p).is_none());
    }

    #[test]
    fn failure_rate_half() {
        let r = failure_rate(50, 100);
        assert!((r - 0.5).abs() < 1e-9);
    }

    #[test]
    fn failure_rate_zero_total_returns_zero() {
        assert!((failure_rate(5, 0) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn failure_rate_no_failures() {
        assert!((failure_rate(0, 100) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn failure_rate_all_failures() {
        assert!((failure_rate(100, 100) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn login_activity_entry_fields_accessible() {
        let e = LoginActivityEntry {
            bucket: "2026-05-01".into(),
            outcome: LoginOutcome::Success,
            count: 42,
        };
        assert_eq!(e.count, 42);
        assert_eq!(e.outcome, LoginOutcome::Success);
    }

    #[test]
    fn login_outcome_serde_roundtrip_success() {
        let json = serde_json::to_string(&LoginOutcome::Success).unwrap();
        let back: LoginOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(back, LoginOutcome::Success);
    }

    #[test]
    fn login_outcome_serde_roundtrip_blocked() {
        let json = serde_json::to_string(&LoginOutcome::Blocked).unwrap();
        let back: LoginOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(back, LoginOutcome::Blocked);
    }

    #[test]
    fn default_less_than_max_since_days() {
        assert!(DEFAULT_SINCE_DAYS < MAX_SINCE_DAYS);
    }

    #[test]
    fn validate_whitespace_tenant_returns_error() {
        let p = LoginActivityParams { tenant_id: "  ".into(), since_days: None, user_id: None };
        assert!(validate_login_activity_params(&p).is_some());
    }

    #[test]
    fn validate_with_user_id_is_ok() {
        let p = LoginActivityParams {
            tenant_id: "t1".into(), since_days: None,
            user_id: Some("alice".into()),
        };
        assert!(validate_login_activity_params(&p).is_none());
    }

    #[test]
    fn failure_rate_one_in_ten() {
        let r = failure_rate(1, 10);
        assert!((r - 0.1).abs() < 1e-9);
    }

    #[test]
    fn login_outcome_serde_roundtrip_failure() {
        let json = serde_json::to_string(&LoginOutcome::Failure).unwrap();
        let back: LoginOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(back, LoginOutcome::Failure);
    }

    #[test]
    fn failure_rate_single_failure_in_one() {
        assert!((failure_rate(1, 1) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn login_outcome_blocked_ne_success() {
        assert_ne!(LoginOutcome::Blocked, LoginOutcome::Success);
    }
}

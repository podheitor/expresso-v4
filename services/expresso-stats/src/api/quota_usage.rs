//! Quota usage analytics types and helpers — sprint #19857.

use serde::{Deserialize, Serialize};

/// Quota usage for a single resource type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaEntry {
    pub resource: String,
    pub used_bytes:  u64,
    pub limit_bytes: u64,
}

impl QuotaEntry {
    /// Usage ratio in [0.0, 1.0]. Returns 0.0 when limit is 0.
    pub fn usage_ratio(&self) -> f64 {
        if self.limit_bytes == 0 { return 0.0; }
        self.used_bytes as f64 / self.limit_bytes as f64
    }

    /// Returns true when usage exceeds 90% of the quota.
    pub fn is_near_limit(&self) -> bool {
        self.usage_ratio() > 0.9
    }

    /// Returns true when usage exactly meets or exceeds the limit.
    pub fn is_exhausted(&self) -> bool {
        self.used_bytes >= self.limit_bytes
    }
}

/// Request parameters for quota usage endpoint.
#[derive(Debug, Deserialize)]
pub struct QuotaUsageParams {
    pub tenant_id: String,
    pub user_id:   Option<String>,
}

pub fn validate_quota_params(p: &QuotaUsageParams) -> Option<String> {
    if p.tenant_id.trim().is_empty() {
        return Some("tenant_id must not be empty".into());
    }
    None
}

/// Known resource identifiers.
pub const RESOURCE_MAILBOX:  &str = "mailbox";
pub const RESOURCE_DRIVE:    &str = "drive";
pub const RESOURCE_CALENDAR: &str = "calendar";

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(used: u64, limit: u64) -> QuotaEntry {
        QuotaEntry { resource: "mailbox".into(), used_bytes: used, limit_bytes: limit }
    }

    #[test]
    fn usage_ratio_half() {
        let e = entry(50, 100);
        assert!((e.usage_ratio() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn usage_ratio_zero_limit_returns_zero() {
        let e = entry(10, 0);
        assert!((e.usage_ratio() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn usage_ratio_full() {
        let e = entry(100, 100);
        assert!((e.usage_ratio() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn usage_ratio_zero_used() {
        let e = entry(0, 1000);
        assert!((e.usage_ratio() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn is_near_limit_true_when_above_90_pct() {
        let e = entry(91, 100);
        assert!(e.is_near_limit());
    }

    #[test]
    fn is_near_limit_false_when_below_90_pct() {
        let e = entry(89, 100);
        assert!(!e.is_near_limit());
    }

    #[test]
    fn is_near_limit_false_exactly_90_pct() {
        let e = entry(90, 100);
        assert!(!e.is_near_limit());
    }

    #[test]
    fn is_exhausted_true_when_used_equals_limit() {
        let e = entry(100, 100);
        assert!(e.is_exhausted());
    }

    #[test]
    fn is_exhausted_true_when_used_exceeds_limit() {
        let e = entry(110, 100);
        assert!(e.is_exhausted());
    }

    #[test]
    fn is_exhausted_false_when_under_limit() {
        let e = entry(99, 100);
        assert!(!e.is_exhausted());
    }

    #[test]
    fn resource_mailbox_constant() {
        assert_eq!(RESOURCE_MAILBOX, "mailbox");
    }

    #[test]
    fn resource_drive_constant() {
        assert_eq!(RESOURCE_DRIVE, "drive");
    }

    #[test]
    fn resource_calendar_constant() {
        assert_eq!(RESOURCE_CALENDAR, "calendar");
    }

    #[test]
    fn validate_empty_tenant_returns_error() {
        let p = QuotaUsageParams { tenant_id: "".into(), user_id: None };
        assert!(validate_quota_params(&p).is_some());
    }

    #[test]
    fn validate_valid_params_returns_none() {
        let p = QuotaUsageParams { tenant_id: "t1".into(), user_id: None };
        assert!(validate_quota_params(&p).is_none());
    }

    #[test]
    fn validate_whitespace_tenant_returns_error() {
        let p = QuotaUsageParams { tenant_id: "  ".into(), user_id: None };
        assert!(validate_quota_params(&p).is_some());
    }

    #[test]
    fn quota_entry_serde_roundtrip() {
        let e = entry(500, 1000);
        let json = serde_json::to_string(&e).unwrap();
        let back: QuotaEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.used_bytes, 500);
    }

    #[test]
    fn quota_entry_resource_field_accessible() {
        let e = QuotaEntry { resource: "drive".into(), used_bytes: 0, limit_bytes: 5_000_000_000 };
        assert_eq!(e.resource, "drive");
    }

    #[test]
    fn all_resource_constants_nonempty() {
        assert!(!RESOURCE_MAILBOX.is_empty());
        assert!(!RESOURCE_DRIVE.is_empty());
        assert!(!RESOURCE_CALENDAR.is_empty());
    }

    #[test]
    fn all_resource_constants_distinct() {
        assert_ne!(RESOURCE_MAILBOX, RESOURCE_DRIVE);
        assert_ne!(RESOURCE_DRIVE, RESOURCE_CALENDAR);
        assert_ne!(RESOURCE_MAILBOX, RESOURCE_CALENDAR);
    }

    #[test]
    fn usage_ratio_never_exceeds_one_in_normal_use() {
        let e = entry(50, 100);
        assert!(e.usage_ratio() <= 1.0);
    }

    #[test]
    fn is_near_limit_true_at_91_pct() {
        let e = entry(91, 100);
        assert!(e.is_near_limit());
    }

    #[test]
    fn validate_with_user_id_is_ok() {
        let p = QuotaUsageParams { tenant_id: "t1".into(), user_id: Some("alice".into()) };
        assert!(validate_quota_params(&p).is_none());
    }

    #[test]
    fn quota_entry_limit_bytes_field() {
        let e = entry(100, 2_000_000);
        assert_eq!(e.limit_bytes, 2_000_000);
    }
}

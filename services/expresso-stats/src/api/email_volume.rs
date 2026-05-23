//! Email volume analytics types and helpers — sprint #19857.

use serde::{Deserialize, Serialize};

/// Granularity for email volume buckets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    #[default]
    Day,
    Week,
    Month,
}

impl std::fmt::Display for Granularity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Granularity::Day   => write!(f, "day"),
            Granularity::Week  => write!(f, "week"),
            Granularity::Month => write!(f, "month"),
        }
    }
}

/// One bucket in the email volume time series.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeEntry {
    pub bucket: String,
    pub sent:   u64,
    pub received: u64,
}

/// Request parameters for the email volume endpoint.
#[derive(Debug, Deserialize)]
pub struct EmailVolumeParams {
    pub tenant_id:   String,
    pub granularity: Option<Granularity>,
    pub since_days:  Option<u32>,
}

pub const DEFAULT_SINCE_DAYS: u32 = 30;
pub const MAX_SINCE_DAYS:     u32 = 365;

pub fn validate_email_volume_params(p: &EmailVolumeParams) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn granularity_day_display() {
        assert_eq!(Granularity::Day.to_string(), "day");
    }

    #[test]
    fn granularity_week_display() {
        assert_eq!(Granularity::Week.to_string(), "week");
    }

    #[test]
    fn granularity_month_display() {
        assert_eq!(Granularity::Month.to_string(), "month");
    }

    #[test]
    fn granularity_default_is_day() {
        assert_eq!(Granularity::default(), Granularity::Day);
    }

    #[test]
    fn granularity_eq_same_variant() {
        assert_eq!(Granularity::Day, Granularity::Day);
    }

    #[test]
    fn granularity_ne_different_variants() {
        assert_ne!(Granularity::Day, Granularity::Month);
    }

    #[test]
    fn default_since_days_is_30() {
        assert_eq!(DEFAULT_SINCE_DAYS, 30);
    }

    #[test]
    fn max_since_days_is_365() {
        assert_eq!(MAX_SINCE_DAYS, 365);
    }

    #[test]
    fn validate_empty_tenant_returns_error() {
        let p = EmailVolumeParams { tenant_id: "".into(), granularity: None, since_days: None };
        assert!(validate_email_volume_params(&p).is_some());
    }

    #[test]
    fn validate_valid_params_returns_none() {
        let p = EmailVolumeParams { tenant_id: "t1".into(), granularity: None, since_days: Some(7) };
        assert!(validate_email_volume_params(&p).is_none());
    }

    #[test]
    fn validate_since_days_zero_returns_error() {
        let p = EmailVolumeParams { tenant_id: "t1".into(), granularity: None, since_days: Some(0) };
        assert!(validate_email_volume_params(&p).is_some());
    }

    #[test]
    fn validate_since_days_above_max_returns_error() {
        let p = EmailVolumeParams { tenant_id: "t1".into(), granularity: None, since_days: Some(MAX_SINCE_DAYS + 1) };
        assert!(validate_email_volume_params(&p).is_some());
    }

    #[test]
    fn validate_since_days_at_max_is_ok() {
        let p = EmailVolumeParams { tenant_id: "t1".into(), granularity: None, since_days: Some(MAX_SINCE_DAYS) };
        assert!(validate_email_volume_params(&p).is_none());
    }

    #[test]
    fn validate_no_since_days_is_ok() {
        let p = EmailVolumeParams { tenant_id: "t1".into(), granularity: None, since_days: None };
        assert!(validate_email_volume_params(&p).is_none());
    }

    #[test]
    fn validate_whitespace_tenant_returns_error() {
        let p = EmailVolumeParams { tenant_id: "  ".into(), granularity: None, since_days: None };
        assert!(validate_email_volume_params(&p).is_some());
    }

    #[test]
    fn volume_entry_fields_accessible() {
        let e = VolumeEntry { bucket: "2026-05-01".into(), sent: 100, received: 250 };
        assert_eq!(e.bucket, "2026-05-01");
        assert_eq!(e.sent, 100);
        assert_eq!(e.received, 250);
    }

    #[test]
    fn volume_entry_serde_roundtrip() {
        let e = VolumeEntry { bucket: "2026-05".into(), sent: 50, received: 150 };
        let json = serde_json::to_string(&e).unwrap();
        let back: VolumeEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sent, 50);
    }

    #[test]
    fn granularity_serde_roundtrip_week() {
        let json = serde_json::to_string(&Granularity::Week).unwrap();
        let back: Granularity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Granularity::Week);
    }

    #[test]
    fn granularity_serde_roundtrip_month() {
        let json = serde_json::to_string(&Granularity::Month).unwrap();
        let back: Granularity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Granularity::Month);
    }

    #[test]
    fn default_less_than_max_since_days() {
        assert!(DEFAULT_SINCE_DAYS < MAX_SINCE_DAYS);
    }

    #[test]
    fn volume_entry_sent_zero() {
        let e = VolumeEntry { bucket: "2026-01".into(), sent: 0, received: 5 };
        assert_eq!(e.sent, 0);
    }

    #[test]
    fn validate_error_message_nonempty_for_invalid() {
        let p = EmailVolumeParams { tenant_id: "".into(), granularity: None, since_days: None };
        let msg = validate_email_volume_params(&p).unwrap();
        assert!(!msg.is_empty());
    }

    #[test]
    fn granularity_serde_roundtrip_day() {
        let json = serde_json::to_string(&Granularity::Day).unwrap();
        let back: Granularity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Granularity::Day);
    }

    #[test]
    fn volume_entry_received_field_accessible() {
        let e = VolumeEntry { bucket: "2026-01".into(), sent: 10, received: 30 };
        assert_eq!(e.received, 30);
    }
}

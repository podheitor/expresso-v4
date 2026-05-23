//! Device statistics types and helpers — sprint #19857.

use serde::{Deserialize, Serialize};

/// Device OS platform categories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Desktop,
    Mobile,
    Tablet,
    Unknown,
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Platform::Desktop => write!(f, "desktop"),
            Platform::Mobile  => write!(f, "mobile"),
            Platform::Tablet  => write!(f, "tablet"),
            Platform::Unknown => write!(f, "unknown"),
        }
    }
}

/// Device statistics entry for a single platform bucket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceStatEntry {
    pub platform:  Platform,
    pub count:     u64,
    pub pct:       f64,
}

/// Request parameters for device stats endpoint.
#[derive(Debug, Deserialize)]
pub struct DeviceStatsParams {
    pub tenant_id: String,
    pub since_days: Option<u32>,
}

/// Default window for device stats (days).
pub const DEFAULT_SINCE_DAYS: u32 = 30;
/// Maximum window for device stats (days).
pub const MAX_SINCE_DAYS: u32 = 365;

pub fn validate_device_params(params: &DeviceStatsParams) -> Option<String> {
    if params.tenant_id.trim().is_empty() {
        return Some("tenant_id must not be empty".into());
    }
    if let Some(d) = params.since_days {
        if d == 0 {
            return Some("since_days must be >= 1".into());
        }
        if d > MAX_SINCE_DAYS {
            return Some(format!("since_days must be <= {}", MAX_SINCE_DAYS));
        }
    }
    None
}

/// Computes percentage from count and total. Returns 0.0 when total is 0.
pub fn compute_pct(count: u64, total: u64) -> f64 {
    if total == 0 { return 0.0; }
    (count as f64 / total as f64) * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_desktop_display() {
        assert_eq!(Platform::Desktop.to_string(), "desktop");
    }

    #[test]
    fn platform_mobile_display() {
        assert_eq!(Platform::Mobile.to_string(), "mobile");
    }

    #[test]
    fn platform_tablet_display() {
        assert_eq!(Platform::Tablet.to_string(), "tablet");
    }

    #[test]
    fn platform_unknown_display() {
        assert_eq!(Platform::Unknown.to_string(), "unknown");
    }

    #[test]
    fn platform_eq_same_variant() {
        assert_eq!(Platform::Desktop, Platform::Desktop);
    }

    #[test]
    fn platform_ne_different_variants() {
        assert_ne!(Platform::Desktop, Platform::Mobile);
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
        let p = DeviceStatsParams { tenant_id: "".into(), since_days: None };
        assert!(validate_device_params(&p).is_some());
    }

    #[test]
    fn validate_valid_params_returns_none() {
        let p = DeviceStatsParams { tenant_id: "t1".into(), since_days: Some(30) };
        assert!(validate_device_params(&p).is_none());
    }

    #[test]
    fn validate_since_days_zero_returns_error() {
        let p = DeviceStatsParams { tenant_id: "t1".into(), since_days: Some(0) };
        assert!(validate_device_params(&p).is_some());
    }

    #[test]
    fn validate_since_days_above_max_returns_error() {
        let p = DeviceStatsParams { tenant_id: "t1".into(), since_days: Some(MAX_SINCE_DAYS + 1) };
        assert!(validate_device_params(&p).is_some());
    }

    #[test]
    fn validate_since_days_at_max_is_ok() {
        let p = DeviceStatsParams { tenant_id: "t1".into(), since_days: Some(MAX_SINCE_DAYS) };
        assert!(validate_device_params(&p).is_none());
    }

    #[test]
    fn validate_no_since_days_is_ok() {
        let p = DeviceStatsParams { tenant_id: "t1".into(), since_days: None };
        assert!(validate_device_params(&p).is_none());
    }

    #[test]
    fn compute_pct_half() {
        let pct = compute_pct(50, 100);
        assert!((pct - 50.0).abs() < 1e-9);
    }

    #[test]
    fn compute_pct_zero_total_returns_zero() {
        assert!((compute_pct(10, 0) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn compute_pct_all_of_total() {
        let pct = compute_pct(100, 100);
        assert!((pct - 100.0).abs() < 1e-9);
    }

    #[test]
    fn compute_pct_zero_count() {
        assert!((compute_pct(0, 100) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn device_stat_entry_fields_accessible() {
        let e = DeviceStatEntry { platform: Platform::Mobile, count: 42, pct: 55.5 };
        assert_eq!(e.count, 42);
        assert!((e.pct - 55.5).abs() < 1e-9);
    }

    #[test]
    fn platform_serde_roundtrip_desktop() {
        let json = serde_json::to_string(&Platform::Desktop).unwrap();
        let back: Platform = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Platform::Desktop);
    }

    #[test]
    fn platform_serde_roundtrip_mobile() {
        let json = serde_json::to_string(&Platform::Mobile).unwrap();
        let back: Platform = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Platform::Mobile);
    }

    #[test]
    fn default_less_than_max_since_days() {
        assert!(DEFAULT_SINCE_DAYS < MAX_SINCE_DAYS);
    }

    #[test]
    fn validate_whitespace_tenant_returns_error() {
        let p = DeviceStatsParams { tenant_id: "  ".into(), since_days: None };
        assert!(validate_device_params(&p).is_some());
    }

    #[test]
    fn compute_pct_quarter() {
        let pct = compute_pct(25, 100);
        assert!((pct - 25.0).abs() < 1e-9);
    }
}

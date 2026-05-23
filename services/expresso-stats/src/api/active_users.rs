use serde::{Deserialize, Serialize};

pub const DEFAULT_DAYS: u32 = 30;
pub const MAX_DAYS: u32 = 365;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActiveUserGranularity {
    Daily,
    Weekly,
    Monthly,
}

impl std::fmt::Display for ActiveUserGranularity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActiveUserGranularity::Daily   => write!(f, "daily"),
            ActiveUserGranularity::Weekly  => write!(f, "weekly"),
            ActiveUserGranularity::Monthly => write!(f, "monthly"),
        }
    }
}

impl ActiveUserGranularity {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "daily"   => Some(ActiveUserGranularity::Daily),
            "weekly"  => Some(ActiveUserGranularity::Weekly),
            "monthly" => Some(ActiveUserGranularity::Monthly),
            _         => None,
        }
    }

    pub fn window_days(&self) -> u32 {
        match self {
            ActiveUserGranularity::Daily   => 1,
            ActiveUserGranularity::Weekly  => 7,
            ActiveUserGranularity::Monthly => 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveUsersParams {
    pub tenant_id:   String,
    #[serde(default = "default_days")]
    pub days:        u32,
    pub granularity: Option<String>,
}

fn default_days() -> u32 {
    DEFAULT_DAYS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActiveUsersRow {
    pub period: String,
    pub count:  i64,
}

pub fn clamp_days(days: u32) -> u32 {
    days.max(1).min(MAX_DAYS)
}

pub fn retention_rate(retained: u64, cohort: u64) -> f64 {
    if cohort == 0 { 0.0 } else { retained as f64 / cohort as f64 * 100.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_days_constant() {
        assert_eq!(DEFAULT_DAYS, 30);
    }

    #[test]
    fn max_days_constant() {
        assert_eq!(MAX_DAYS, 365);
    }

    #[test]
    fn granularity_display_daily() {
        assert_eq!(ActiveUserGranularity::Daily.to_string(), "daily");
    }

    #[test]
    fn granularity_display_weekly() {
        assert_eq!(ActiveUserGranularity::Weekly.to_string(), "weekly");
    }

    #[test]
    fn granularity_display_monthly() {
        assert_eq!(ActiveUserGranularity::Monthly.to_string(), "monthly");
    }

    #[test]
    fn from_str_daily() {
        assert_eq!(ActiveUserGranularity::from_str("daily"), Some(ActiveUserGranularity::Daily));
    }

    #[test]
    fn from_str_weekly() {
        assert_eq!(ActiveUserGranularity::from_str("weekly"), Some(ActiveUserGranularity::Weekly));
    }

    #[test]
    fn from_str_monthly_uppercase() {
        assert_eq!(ActiveUserGranularity::from_str("MONTHLY"), Some(ActiveUserGranularity::Monthly));
    }

    #[test]
    fn from_str_unknown_returns_none() {
        assert_eq!(ActiveUserGranularity::from_str("hourly"), None);
    }

    #[test]
    fn window_days_daily() {
        assert_eq!(ActiveUserGranularity::Daily.window_days(), 1);
    }

    #[test]
    fn window_days_weekly() {
        assert_eq!(ActiveUserGranularity::Weekly.window_days(), 7);
    }

    #[test]
    fn window_days_monthly() {
        assert_eq!(ActiveUserGranularity::Monthly.window_days(), 30);
    }

    #[test]
    fn retention_rate_zero_cohort() {
        assert_eq!(retention_rate(10, 0), 0.0);
    }

    #[test]
    fn retention_rate_half() {
        assert!((retention_rate(50, 100) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn retention_rate_full() {
        assert!((retention_rate(100, 100) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn retention_rate_zero_retained() {
        assert_eq!(retention_rate(0, 100), 0.0);
    }

    #[test]
    fn clamp_days_below_min() {
        assert_eq!(clamp_days(0), 1);
    }

    #[test]
    fn clamp_days_above_max() {
        assert_eq!(clamp_days(400), MAX_DAYS);
    }

    #[test]
    fn clamp_days_within_range() {
        assert_eq!(clamp_days(90), 90);
    }

    #[test]
    fn default_days_fn_returns_30() {
        assert_eq!(default_days(), 30);
    }

    #[test]
    fn granularity_equality() {
        assert_eq!(ActiveUserGranularity::Daily, ActiveUserGranularity::Daily);
        assert_ne!(ActiveUserGranularity::Daily, ActiveUserGranularity::Weekly);
    }

    #[test]
    fn serde_roundtrip_granularity() {
        let g = ActiveUserGranularity::Weekly;
        let json = serde_json::to_string(&g).unwrap();
        let back: ActiveUserGranularity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ActiveUserGranularity::Weekly);
    }

    #[test]
    fn active_users_row_serde_roundtrip() {
        let row = ActiveUsersRow { period: "2026-05-01".into(), count: 350 };
        let json = serde_json::to_string(&row).unwrap();
        let back: ActiveUsersRow = serde_json::from_str(&json).unwrap();
        assert_eq!(back, row);
    }

    #[test]
    fn max_days_is_365() {
        assert_eq!(MAX_DAYS, 365);
    }

    #[test]
    fn retention_rate_three_quarters() {
        assert!((retention_rate(75, 100) - 75.0).abs() < 1e-9);
    }

    #[test]
    fn granularity_debug_contains_variant_name() {
        assert!(format!("{:?}", ActiveUserGranularity::Monthly).contains("Monthly"));
    }
}

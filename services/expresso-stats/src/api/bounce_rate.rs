use serde::{Deserialize, Serialize};

pub const DEFAULT_DAYS: u32 = 30;
pub const MAX_DAYS: u32 = 365;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BounceKind {
    HardBounce,
    SoftBounce,
    SpamReport,
    Unsubscribed,
}

impl std::fmt::Display for BounceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BounceKind::HardBounce   => write!(f, "hard_bounce"),
            BounceKind::SoftBounce   => write!(f, "soft_bounce"),
            BounceKind::SpamReport   => write!(f, "spam_report"),
            BounceKind::Unsubscribed => write!(f, "unsubscribed"),
        }
    }
}

impl BounceKind {
    pub fn is_permanent(&self) -> bool {
        matches!(self, BounceKind::HardBounce | BounceKind::Unsubscribed)
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "hard_bounce"  => Some(BounceKind::HardBounce),
            "soft_bounce"  => Some(BounceKind::SoftBounce),
            "spam_report"  => Some(BounceKind::SpamReport),
            "unsubscribed" => Some(BounceKind::Unsubscribed),
            _              => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BounceRateParams {
    pub tenant_id:   String,
    #[serde(default = "default_days")]
    pub days:        u32,
    pub bounce_kind: Option<String>,
}

fn default_days() -> u32 {
    DEFAULT_DAYS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BounceRateRow {
    pub date:   String,
    pub kind:   String,
    pub count:  i64,
    pub sent:   i64,
}

impl BounceRateRow {
    pub fn rate(&self) -> f64 {
        if self.sent == 0 { 0.0 } else { self.count as f64 / self.sent as f64 * 100.0 }
    }
}

pub fn clamp_days(days: u32) -> u32 {
    days.max(1).min(MAX_DAYS)
}

pub fn overall_bounce_rate(total_bounced: u64, total_sent: u64) -> f64 {
    if total_sent == 0 { 0.0 } else { total_bounced as f64 / total_sent as f64 * 100.0 }
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
    fn bounce_display_hard() {
        assert_eq!(BounceKind::HardBounce.to_string(), "hard_bounce");
    }

    #[test]
    fn bounce_display_soft() {
        assert_eq!(BounceKind::SoftBounce.to_string(), "soft_bounce");
    }

    #[test]
    fn bounce_display_spam_report() {
        assert_eq!(BounceKind::SpamReport.to_string(), "spam_report");
    }

    #[test]
    fn bounce_display_unsubscribed() {
        assert_eq!(BounceKind::Unsubscribed.to_string(), "unsubscribed");
    }

    #[test]
    fn hard_bounce_is_permanent() {
        assert!(BounceKind::HardBounce.is_permanent());
    }

    #[test]
    fn soft_bounce_not_permanent() {
        assert!(!BounceKind::SoftBounce.is_permanent());
    }

    #[test]
    fn unsubscribed_is_permanent() {
        assert!(BounceKind::Unsubscribed.is_permanent());
    }

    #[test]
    fn spam_report_not_permanent() {
        assert!(!BounceKind::SpamReport.is_permanent());
    }

    #[test]
    fn from_str_hard_bounce() {
        assert_eq!(BounceKind::from_str("hard_bounce"), Some(BounceKind::HardBounce));
    }

    #[test]
    fn from_str_soft_bounce() {
        assert_eq!(BounceKind::from_str("soft_bounce"), Some(BounceKind::SoftBounce));
    }

    #[test]
    fn from_str_uppercase() {
        assert_eq!(BounceKind::from_str("SPAM_REPORT"), Some(BounceKind::SpamReport));
    }

    #[test]
    fn from_str_unknown_returns_none() {
        assert_eq!(BounceKind::from_str("delayed"), None);
    }

    #[test]
    fn bounce_rate_row_rate_computed() {
        let row = BounceRateRow { date: "2026-05-01".into(), kind: "hard_bounce".into(), count: 10, sent: 1000 };
        assert!((row.rate() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn bounce_rate_row_rate_zero_sent() {
        let row = BounceRateRow { date: "2026-05-01".into(), kind: "hard_bounce".into(), count: 5, sent: 0 };
        assert_eq!(row.rate(), 0.0);
    }

    #[test]
    fn overall_bounce_rate_zero_sent() {
        assert_eq!(overall_bounce_rate(10, 0), 0.0);
    }

    #[test]
    fn overall_bounce_rate_computed() {
        assert!((overall_bounce_rate(100, 1000) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn clamp_days_below_min() {
        assert_eq!(clamp_days(0), 1);
    }

    #[test]
    fn clamp_days_above_max() {
        assert_eq!(clamp_days(MAX_DAYS + 50), MAX_DAYS);
    }

    #[test]
    fn clamp_days_within_range() {
        assert_eq!(clamp_days(30), 30);
    }

    #[test]
    fn default_days_fn_returns_30() {
        assert_eq!(default_days(), 30);
    }

    #[test]
    fn bounce_kind_equality() {
        assert_eq!(BounceKind::HardBounce, BounceKind::HardBounce);
        assert_ne!(BounceKind::HardBounce, BounceKind::SoftBounce);
    }

    #[test]
    fn serde_roundtrip_bounce_kind() {
        let k = BounceKind::SpamReport;
        let json = serde_json::to_string(&k).unwrap();
        let back: BounceKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, BounceKind::SpamReport);
    }
}

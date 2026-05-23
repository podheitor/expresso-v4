use serde::{Deserialize, Serialize};

pub const DEFAULT_DAYS: u32 = 7;
pub const MAX_DAYS: u32 = 90;
pub const P50_PERCENTILE: f64 = 50.0;
pub const P95_PERCENTILE: f64 = 95.0;
pub const P99_PERCENTILE: f64 = 99.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponseTimeMetric {
    P50Ms,
    P95Ms,
    P99Ms,
    AvgMs,
    MaxMs,
    RequestCount,
}

impl std::fmt::Display for ResponseTimeMetric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResponseTimeMetric::P50Ms        => write!(f, "p50_ms"),
            ResponseTimeMetric::P95Ms        => write!(f, "p95_ms"),
            ResponseTimeMetric::P99Ms        => write!(f, "p99_ms"),
            ResponseTimeMetric::AvgMs        => write!(f, "avg_ms"),
            ResponseTimeMetric::MaxMs        => write!(f, "max_ms"),
            ResponseTimeMetric::RequestCount => write!(f, "request_count"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceName {
    Mail,
    Calendar,
    Drive,
    Meet,
    Chat,
    Auth,
    Search,
}

impl std::fmt::Display for ServiceName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceName::Mail     => write!(f, "mail"),
            ServiceName::Calendar => write!(f, "calendar"),
            ServiceName::Drive    => write!(f, "drive"),
            ServiceName::Meet     => write!(f, "meet"),
            ServiceName::Chat     => write!(f, "chat"),
            ServiceName::Auth     => write!(f, "auth"),
            ServiceName::Search   => write!(f, "search"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseTimeParams {
    pub tenant_id: String,
    #[serde(default = "default_days")]
    pub days:      u32,
    pub service:   Option<String>,
}

fn default_days() -> u32 {
    DEFAULT_DAYS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResponseTimeRow {
    pub date:    String,
    pub service: String,
    pub p50_ms:  f64,
    pub p95_ms:  f64,
    pub p99_ms:  f64,
    pub avg_ms:  f64,
}

pub fn clamp_days(days: u32) -> u32 {
    days.max(1).min(MAX_DAYS)
}

pub fn is_within_slo(p99_ms: f64, slo_ms: f64) -> bool {
    p99_ms <= slo_ms
}

pub fn percentile_from_sorted(values: &[f64], pct: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let idx = ((pct / 100.0) * values.len() as f64).ceil() as usize;
    values[(idx.saturating_sub(1)).min(values.len() - 1)]
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
    fn p50_percentile_constant() {
        assert!((P50_PERCENTILE - 50.0).abs() < 1e-9);
    }

    #[test]
    fn p95_percentile_constant() {
        assert!((P95_PERCENTILE - 95.0).abs() < 1e-9);
    }

    #[test]
    fn p99_percentile_constant() {
        assert!((P99_PERCENTILE - 99.0).abs() < 1e-9);
    }

    #[test]
    fn metric_display_p50() {
        assert_eq!(ResponseTimeMetric::P50Ms.to_string(), "p50_ms");
    }

    #[test]
    fn metric_display_p95() {
        assert_eq!(ResponseTimeMetric::P95Ms.to_string(), "p95_ms");
    }

    #[test]
    fn metric_display_p99() {
        assert_eq!(ResponseTimeMetric::P99Ms.to_string(), "p99_ms");
    }

    #[test]
    fn metric_display_avg() {
        assert_eq!(ResponseTimeMetric::AvgMs.to_string(), "avg_ms");
    }

    #[test]
    fn metric_display_max() {
        assert_eq!(ResponseTimeMetric::MaxMs.to_string(), "max_ms");
    }

    #[test]
    fn metric_display_request_count() {
        assert_eq!(ResponseTimeMetric::RequestCount.to_string(), "request_count");
    }

    #[test]
    fn service_display_mail() {
        assert_eq!(ServiceName::Mail.to_string(), "mail");
    }

    #[test]
    fn service_display_auth() {
        assert_eq!(ServiceName::Auth.to_string(), "auth");
    }

    #[test]
    fn service_display_search() {
        assert_eq!(ServiceName::Search.to_string(), "search");
    }

    #[test]
    fn is_within_slo_true() {
        assert!(is_within_slo(200.0, 500.0));
    }

    #[test]
    fn is_within_slo_false() {
        assert!(!is_within_slo(600.0, 500.0));
    }

    #[test]
    fn is_within_slo_exactly_at_boundary() {
        assert!(is_within_slo(500.0, 500.0));
    }

    #[test]
    fn clamp_days_below_min() {
        assert_eq!(clamp_days(0), 1);
    }

    #[test]
    fn clamp_days_above_max() {
        assert_eq!(clamp_days(MAX_DAYS + 10), MAX_DAYS);
    }

    #[test]
    fn clamp_days_within_range() {
        assert_eq!(clamp_days(14), 14);
    }

    #[test]
    fn percentile_empty_returns_zero() {
        assert_eq!(percentile_from_sorted(&[], 50.0), 0.0);
    }

    #[test]
    fn percentile_p50_single_element() {
        assert!((percentile_from_sorted(&[42.0], 50.0) - 42.0).abs() < 1e-9);
    }

    #[test]
    fn percentile_p100_returns_last() {
        let v = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert!((percentile_from_sorted(&v, 100.0) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn metric_equality() {
        assert_eq!(ResponseTimeMetric::P99Ms, ResponseTimeMetric::P99Ms);
        assert_ne!(ResponseTimeMetric::P99Ms, ResponseTimeMetric::P50Ms);
    }

    #[test]
    fn default_days_fn_returns_7() {
        assert_eq!(default_days(), 7);
    }

    #[test]
    fn serde_roundtrip_metric() {
        let m = ResponseTimeMetric::P95Ms;
        let json = serde_json::to_string(&m).unwrap();
        let back: ResponseTimeMetric = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ResponseTimeMetric::P95Ms);
    }
}

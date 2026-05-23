//! Prometheus metrics for the stats service — sprint #19857.

use serde::{Deserialize, Serialize};

/// Supported aggregation granularities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    Hour,
    Day,
    Week,
    Month,
}

impl Granularity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hour  => "hour",
            Self::Day   => "day",
            Self::Week  => "week",
            Self::Month => "month",
        }
    }

    pub fn seconds(&self) -> u64 {
        match self {
            Self::Hour  => 3_600,
            Self::Day   => 86_400,
            Self::Week  => 604_800,
            Self::Month => 2_592_000,
        }
    }
}

impl std::fmt::Display for Granularity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single time-series data point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataPoint {
    pub bucket:    String,
    pub value:     f64,
}

/// Aggregate result envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSeries {
    pub metric:       String,
    pub granularity:  Granularity,
    pub points:       Vec<DataPoint>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn granularity_hour_as_str() {
        assert_eq!(Granularity::Hour.as_str(), "hour");
    }

    #[test]
    fn granularity_day_as_str() {
        assert_eq!(Granularity::Day.as_str(), "day");
    }

    #[test]
    fn granularity_week_as_str() {
        assert_eq!(Granularity::Week.as_str(), "week");
    }

    #[test]
    fn granularity_month_as_str() {
        assert_eq!(Granularity::Month.as_str(), "month");
    }

    #[test]
    fn granularity_hour_seconds() {
        assert_eq!(Granularity::Hour.seconds(), 3_600);
    }

    #[test]
    fn granularity_day_seconds() {
        assert_eq!(Granularity::Day.seconds(), 86_400);
    }

    #[test]
    fn granularity_week_seconds() {
        assert_eq!(Granularity::Week.seconds(), 604_800);
    }

    #[test]
    fn granularity_month_seconds() {
        assert_eq!(Granularity::Month.seconds(), 2_592_000);
    }

    #[test]
    fn granularity_display_hour() {
        assert_eq!(format!("{}", Granularity::Hour), "hour");
    }

    #[test]
    fn granularity_display_day() {
        assert_eq!(format!("{}", Granularity::Day), "day");
    }

    #[test]
    fn granularity_serde_roundtrip_day() {
        let s = serde_json::to_string(&Granularity::Day).unwrap();
        let back: Granularity = serde_json::from_str(&s).unwrap();
        assert_eq!(back, Granularity::Day);
    }

    #[test]
    fn granularity_serde_roundtrip_hour() {
        let s = serde_json::to_string(&Granularity::Hour).unwrap();
        let back: Granularity = serde_json::from_str(&s).unwrap();
        assert_eq!(back, Granularity::Hour);
    }

    #[test]
    fn granularity_equality() {
        assert_eq!(Granularity::Week, Granularity::Week);
        assert_ne!(Granularity::Week, Granularity::Day);
    }

    #[test]
    fn data_point_serde_roundtrip() {
        let p = DataPoint { bucket: "2026-05-22".into(), value: 42.0 };
        let s = serde_json::to_string(&p).unwrap();
        let back: DataPoint = serde_json::from_str(&s).unwrap();
        assert_eq!(back.bucket, "2026-05-22");
        assert!((back.value - 42.0).abs() < 1e-9);
    }

    #[test]
    fn metric_series_serde_roundtrip() {
        let ms = MetricSeries {
            metric:      "mail.sent".into(),
            granularity: Granularity::Day,
            points:      vec![DataPoint { bucket: "2026-05-01".into(), value: 100.0 }],
        };
        let s = serde_json::to_string(&ms).unwrap();
        let back: MetricSeries = serde_json::from_str(&s).unwrap();
        assert_eq!(back.metric, "mail.sent");
        assert_eq!(back.granularity, Granularity::Day);
        assert_eq!(back.points.len(), 1);
    }

    #[test]
    fn metric_series_empty_points() {
        let ms = MetricSeries {
            metric:      "chat.messages".into(),
            granularity: Granularity::Hour,
            points:      vec![],
        };
        assert!(ms.points.is_empty());
    }

    #[test]
    fn data_point_clone_preserves_value() {
        let p = DataPoint { bucket: "2026-01".into(), value: 7.5 };
        let c = p.clone();
        assert!((c.value - 7.5).abs() < 1e-9);
    }

    #[test]
    fn granularity_copy_trait() {
        let g = Granularity::Month;
        let g2 = g;
        assert_eq!(g, g2);
    }

    #[test]
    fn granularity_debug_contains_variant_name() {
        let s = format!("{:?}", Granularity::Week);
        assert!(s.contains("Week"));
    }

    #[test]
    fn data_point_bucket_nonempty() {
        let p = DataPoint { bucket: "2026-Q1".into(), value: 0.0 };
        assert!(!p.bucket.is_empty());
    }

    #[test]
    fn metric_series_metric_nonempty() {
        let ms = MetricSeries {
            metric:      "active_users".into(),
            granularity: Granularity::Week,
            points:      vec![],
        };
        assert!(!ms.metric.is_empty());
    }

    #[test]
    fn granularity_day_seconds_divisible_by_hour() {
        assert_eq!(Granularity::Day.seconds() % Granularity::Hour.seconds(), 0);
    }

    #[test]
    fn granularity_week_seconds_divisible_by_day() {
        assert_eq!(Granularity::Week.seconds() % Granularity::Day.seconds(), 0);
    }

    #[test]
    fn granularity_month_display() {
        assert_eq!(format!("{}", Granularity::Month), "month");
    }

    #[test]
    fn granularity_serde_roundtrip_week() {
        let s = serde_json::to_string(&Granularity::Week).unwrap();
        let back: Granularity = serde_json::from_str(&s).unwrap();
        assert_eq!(back, Granularity::Week);
    }
}

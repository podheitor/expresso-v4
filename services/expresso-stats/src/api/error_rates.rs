use serde::{Deserialize, Serialize};

pub const DEFAULT_DAYS: u32 = 7;
pub const MAX_DAYS: u32 = 90;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpStatusClass {
    Success2xx,
    Redirect3xx,
    ClientError4xx,
    ServerError5xx,
}

impl std::fmt::Display for HttpStatusClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpStatusClass::Success2xx    => write!(f, "2xx"),
            HttpStatusClass::Redirect3xx   => write!(f, "3xx"),
            HttpStatusClass::ClientError4xx => write!(f, "4xx"),
            HttpStatusClass::ServerError5xx => write!(f, "5xx"),
        }
    }
}

impl HttpStatusClass {
    pub fn from_status(code: u16) -> Option<Self> {
        match code {
            200..=299 => Some(HttpStatusClass::Success2xx),
            300..=399 => Some(HttpStatusClass::Redirect3xx),
            400..=499 => Some(HttpStatusClass::ClientError4xx),
            500..=599 => Some(HttpStatusClass::ServerError5xx),
            _         => None,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, HttpStatusClass::ClientError4xx | HttpStatusClass::ServerError5xx)
    }

    pub fn is_server_error(&self) -> bool {
        matches!(self, HttpStatusClass::ServerError5xx)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorRatesParams {
    pub tenant_id:     String,
    #[serde(default = "default_days")]
    pub days:          u32,
    pub service:       Option<String>,
    pub status_class:  Option<String>,
}

fn default_days() -> u32 {
    DEFAULT_DAYS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorRateRow {
    pub date:         String,
    pub service:      String,
    pub status_class: String,
    pub count:        i64,
    pub total:        i64,
}

impl ErrorRateRow {
    pub fn rate(&self) -> f64 {
        if self.total == 0 { 0.0 } else { self.count as f64 / self.total as f64 * 100.0 }
    }
}

pub fn clamp_days(days: u32) -> u32 {
    days.max(1).min(MAX_DAYS)
}

pub fn error_rate(errors: u64, total: u64) -> f64 {
    if total == 0 { 0.0 } else { errors as f64 / total as f64 * 100.0 }
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
    fn status_class_display_2xx() {
        assert_eq!(HttpStatusClass::Success2xx.to_string(), "2xx");
    }

    #[test]
    fn status_class_display_3xx() {
        assert_eq!(HttpStatusClass::Redirect3xx.to_string(), "3xx");
    }

    #[test]
    fn status_class_display_4xx() {
        assert_eq!(HttpStatusClass::ClientError4xx.to_string(), "4xx");
    }

    #[test]
    fn status_class_display_5xx() {
        assert_eq!(HttpStatusClass::ServerError5xx.to_string(), "5xx");
    }

    #[test]
    fn from_status_200() {
        assert_eq!(HttpStatusClass::from_status(200), Some(HttpStatusClass::Success2xx));
    }

    #[test]
    fn from_status_301() {
        assert_eq!(HttpStatusClass::from_status(301), Some(HttpStatusClass::Redirect3xx));
    }

    #[test]
    fn from_status_404() {
        assert_eq!(HttpStatusClass::from_status(404), Some(HttpStatusClass::ClientError4xx));
    }

    #[test]
    fn from_status_500() {
        assert_eq!(HttpStatusClass::from_status(500), Some(HttpStatusClass::ServerError5xx));
    }

    #[test]
    fn from_status_100_returns_none() {
        assert_eq!(HttpStatusClass::from_status(100), None);
    }

    #[test]
    fn from_status_600_returns_none() {
        assert_eq!(HttpStatusClass::from_status(600), None);
    }

    #[test]
    fn client_error_is_error() {
        assert!(HttpStatusClass::ClientError4xx.is_error());
    }

    #[test]
    fn server_error_is_error() {
        assert!(HttpStatusClass::ServerError5xx.is_error());
    }

    #[test]
    fn success_not_error() {
        assert!(!HttpStatusClass::Success2xx.is_error());
    }

    #[test]
    fn redirect_not_error() {
        assert!(!HttpStatusClass::Redirect3xx.is_error());
    }

    #[test]
    fn server_error_is_server_error() {
        assert!(HttpStatusClass::ServerError5xx.is_server_error());
    }

    #[test]
    fn client_error_not_server_error() {
        assert!(!HttpStatusClass::ClientError4xx.is_server_error());
    }

    #[test]
    fn error_rate_zero_total() {
        assert_eq!(error_rate(10, 0), 0.0);
    }

    #[test]
    fn error_rate_1_percent() {
        assert!((error_rate(1, 100) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn error_rate_row_rate_computed() {
        let row = ErrorRateRow { date: "2026-05-01".into(), service: "mail".into(),
            status_class: "5xx".into(), count: 5, total: 1000 };
        assert!((row.rate() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn error_rate_row_rate_zero_total() {
        let row = ErrorRateRow { date: "2026-05-01".into(), service: "mail".into(),
            status_class: "5xx".into(), count: 5, total: 0 };
        assert_eq!(row.rate(), 0.0);
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
    fn default_days_fn_returns_7() {
        assert_eq!(default_days(), 7);
    }

    #[test]
    fn status_class_equality() {
        assert_eq!(HttpStatusClass::Success2xx, HttpStatusClass::Success2xx);
        assert_ne!(HttpStatusClass::Success2xx, HttpStatusClass::ServerError5xx);
    }

    #[test]
    fn serde_roundtrip_status_class() {
        let c = HttpStatusClass::ClientError4xx;
        let json = serde_json::to_string(&c).unwrap();
        let back: HttpStatusClass = serde_json::from_str(&json).unwrap();
        assert_eq!(back, HttpStatusClass::ClientError4xx);
    }
}

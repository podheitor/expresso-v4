use serde::{Deserialize, Serialize};

pub const DEFAULT_DAYS: u32 = 30;
pub const MAX_DAYS: u32 = 365;
pub const MAX_TOP_N: usize = 50;
pub const DEFAULT_TOP_N: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GeoGrouping {
    Country,
    Region,
    City,
    Continent,
}

impl std::fmt::Display for GeoGrouping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeoGrouping::Country   => write!(f, "country"),
            GeoGrouping::Region    => write!(f, "region"),
            GeoGrouping::City      => write!(f, "city"),
            GeoGrouping::Continent => write!(f, "continent"),
        }
    }
}

impl GeoGrouping {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "country"   => Some(GeoGrouping::Country),
            "region"    => Some(GeoGrouping::Region),
            "city"      => Some(GeoGrouping::City),
            "continent" => Some(GeoGrouping::Continent),
            _           => None,
        }
    }

    pub fn is_fine_grained(&self) -> bool {
        matches!(self, GeoGrouping::City | GeoGrouping::Region)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoDistributionParams {
    pub tenant_id: String,
    #[serde(default = "default_days")]
    pub days:      u32,
    pub grouping:  Option<String>,
    #[serde(default = "default_top_n")]
    pub top_n:     usize,
}

fn default_days() -> u32 {
    DEFAULT_DAYS
}

fn default_top_n() -> usize {
    DEFAULT_TOP_N
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeoRow {
    pub location: String,
    pub grouping: String,
    pub count:    i64,
    pub pct:      f64,
}

pub fn clamp_days(days: u32) -> u32 {
    days.max(1).min(MAX_DAYS)
}

pub fn clamp_top_n(n: usize) -> usize {
    n.max(1).min(MAX_TOP_N)
}

pub fn compute_pct(count: u64, total: u64) -> f64 {
    if total == 0 { 0.0 } else { count as f64 / total as f64 * 100.0 }
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
    fn max_top_n_constant() {
        assert_eq!(MAX_TOP_N, 50);
    }

    #[test]
    fn default_top_n_constant() {
        assert_eq!(DEFAULT_TOP_N, 10);
    }

    #[test]
    fn geo_grouping_display_country() {
        assert_eq!(GeoGrouping::Country.to_string(), "country");
    }

    #[test]
    fn geo_grouping_display_region() {
        assert_eq!(GeoGrouping::Region.to_string(), "region");
    }

    #[test]
    fn geo_grouping_display_city() {
        assert_eq!(GeoGrouping::City.to_string(), "city");
    }

    #[test]
    fn geo_grouping_display_continent() {
        assert_eq!(GeoGrouping::Continent.to_string(), "continent");
    }

    #[test]
    fn from_str_country() {
        assert_eq!(GeoGrouping::from_str("country"), Some(GeoGrouping::Country));
    }

    #[test]
    fn from_str_city_uppercase() {
        assert_eq!(GeoGrouping::from_str("CITY"), Some(GeoGrouping::City));
    }

    #[test]
    fn from_str_unknown_returns_none() {
        assert_eq!(GeoGrouping::from_str("district"), None);
    }

    #[test]
    fn city_is_fine_grained() {
        assert!(GeoGrouping::City.is_fine_grained());
    }

    #[test]
    fn region_is_fine_grained() {
        assert!(GeoGrouping::Region.is_fine_grained());
    }

    #[test]
    fn country_not_fine_grained() {
        assert!(!GeoGrouping::Country.is_fine_grained());
    }

    #[test]
    fn continent_not_fine_grained() {
        assert!(!GeoGrouping::Continent.is_fine_grained());
    }

    #[test]
    fn compute_pct_zero_total() {
        assert_eq!(compute_pct(10, 0), 0.0);
    }

    #[test]
    fn compute_pct_half() {
        assert!((compute_pct(50, 100) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn compute_pct_full() {
        assert!((compute_pct(100, 100) - 100.0).abs() < 1e-9);
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
    fn clamp_top_n_below_min() {
        assert_eq!(clamp_top_n(0), 1);
    }

    #[test]
    fn clamp_top_n_above_max() {
        assert_eq!(clamp_top_n(MAX_TOP_N + 5), MAX_TOP_N);
    }

    #[test]
    fn clamp_top_n_within_range() {
        assert_eq!(clamp_top_n(15), 15);
    }

    #[test]
    fn default_days_fn_returns_30() {
        assert_eq!(default_days(), 30);
    }

    #[test]
    fn default_top_n_fn_returns_10() {
        assert_eq!(default_top_n(), 10);
    }

    #[test]
    fn geo_grouping_equality() {
        assert_eq!(GeoGrouping::Country, GeoGrouping::Country);
        assert_ne!(GeoGrouping::Country, GeoGrouping::City);
    }

    #[test]
    fn serde_roundtrip_grouping() {
        let g = GeoGrouping::Continent;
        let json = serde_json::to_string(&g).unwrap();
        let back: GeoGrouping = serde_json::from_str(&json).unwrap();
        assert_eq!(back, GeoGrouping::Continent);
    }
}

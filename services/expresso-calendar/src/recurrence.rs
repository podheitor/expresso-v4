//! iCalendar RRULE recurrence utilities — pure-logic helpers.

/// Maximum number of occurrences expanded from a single RRULE to prevent
/// runaway expansion.
pub const MAX_OCCURRENCES: usize = 10_000;

/// Known FREQ values per RFC 5545 §3.3.10.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frequency {
    Secondly,
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Frequency {
    /// Parse from the string value used in RRULE FREQ= component.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "SECONDLY" => Some(Self::Secondly),
            "MINUTELY" => Some(Self::Minutely),
            "HOURLY"   => Some(Self::Hourly),
            "DAILY"    => Some(Self::Daily),
            "WEEKLY"   => Some(Self::Weekly),
            "MONTHLY"  => Some(Self::Monthly),
            "YEARLY"   => Some(Self::Yearly),
            _          => None,
        }
    }

    /// Returns the canonical uppercase string for this frequency.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Secondly => "SECONDLY",
            Self::Minutely => "MINUTELY",
            Self::Hourly   => "HOURLY",
            Self::Daily    => "DAILY",
            Self::Weekly   => "WEEKLY",
            Self::Monthly  => "MONTHLY",
            Self::Yearly   => "YEARLY",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_occurrences_value() {
        assert_eq!(MAX_OCCURRENCES, 10_000);
    }

    #[test]
    fn frequency_from_str_daily() {
        assert_eq!(Frequency::from_str("DAILY"), Some(Frequency::Daily));
    }

    #[test]
    fn frequency_from_str_weekly() {
        assert_eq!(Frequency::from_str("WEEKLY"), Some(Frequency::Weekly));
    }

    #[test]
    fn frequency_from_str_monthly() {
        assert_eq!(Frequency::from_str("MONTHLY"), Some(Frequency::Monthly));
    }

    #[test]
    fn frequency_from_str_yearly() {
        assert_eq!(Frequency::from_str("YEARLY"), Some(Frequency::Yearly));
    }

    #[test]
    fn frequency_from_str_hourly() {
        assert_eq!(Frequency::from_str("HOURLY"), Some(Frequency::Hourly));
    }

    #[test]
    fn frequency_from_str_minutely() {
        assert_eq!(Frequency::from_str("MINUTELY"), Some(Frequency::Minutely));
    }

    #[test]
    fn frequency_from_str_secondly() {
        assert_eq!(Frequency::from_str("SECONDLY"), Some(Frequency::Secondly));
    }

    #[test]
    fn frequency_from_str_lowercase_accepted() {
        assert_eq!(Frequency::from_str("daily"), Some(Frequency::Daily));
    }

    #[test]
    fn frequency_from_str_mixed_case_accepted() {
        assert_eq!(Frequency::from_str("wEeKlY"), Some(Frequency::Weekly));
    }

    #[test]
    fn frequency_from_str_unknown_returns_none() {
        assert!(Frequency::from_str("BIWEEKLY").is_none());
    }

    #[test]
    fn frequency_from_str_empty_returns_none() {
        assert!(Frequency::from_str("").is_none());
    }

    #[test]
    fn frequency_as_str_daily() {
        assert_eq!(Frequency::Daily.as_str(), "DAILY");
    }

    #[test]
    fn frequency_as_str_weekly() {
        assert_eq!(Frequency::Weekly.as_str(), "WEEKLY");
    }

    #[test]
    fn frequency_as_str_monthly() {
        assert_eq!(Frequency::Monthly.as_str(), "MONTHLY");
    }

    #[test]
    fn frequency_as_str_yearly() {
        assert_eq!(Frequency::Yearly.as_str(), "YEARLY");
    }

    #[test]
    fn frequency_as_str_is_uppercase() {
        for f in [Frequency::Secondly, Frequency::Minutely, Frequency::Hourly,
                  Frequency::Daily, Frequency::Weekly, Frequency::Monthly, Frequency::Yearly] {
            let s = f.as_str();
            assert_eq!(s, s.to_uppercase());
        }
    }

    #[test]
    fn frequency_roundtrip_daily() {
        let f = Frequency::Daily;
        assert_eq!(Frequency::from_str(f.as_str()), Some(f));
    }

    #[test]
    fn frequency_roundtrip_weekly() {
        let f = Frequency::Weekly;
        assert_eq!(Frequency::from_str(f.as_str()), Some(f));
    }

    #[test]
    fn frequency_equality() {
        assert_eq!(Frequency::Daily, Frequency::Daily);
        assert_ne!(Frequency::Daily, Frequency::Weekly);
    }

    #[test]
    fn frequency_clone_equals_original() {
        let f = Frequency::Monthly;
        assert_eq!(f, f.clone());
    }

    #[test]
    fn max_occurrences_is_positive() {
        assert!(MAX_OCCURRENCES > 0);
    }

    #[test]
    fn frequency_from_str_garbage_returns_none() {
        assert!(Frequency::from_str("NEVER").is_none());
    }

    #[test]
    fn frequency_as_str_nonempty_for_all_variants() {
        for f in [Frequency::Secondly, Frequency::Minutely, Frequency::Hourly,
                  Frequency::Daily, Frequency::Weekly, Frequency::Monthly, Frequency::Yearly] {
            assert!(!f.as_str().is_empty());
        }
    }
}

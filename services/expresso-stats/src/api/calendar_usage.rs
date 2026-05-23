use serde::{Deserialize, Serialize};

pub const DEFAULT_DAYS: u32 = 30;
pub const MAX_DAYS: u32 = 365;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalendarMetric {
    EventsCreated,
    EventsDeleted,
    InvitesSent,
    InvitesAccepted,
    InvitesDeclined,
    ActiveCalendars,
}

impl std::fmt::Display for CalendarMetric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CalendarMetric::EventsCreated    => write!(f, "events_created"),
            CalendarMetric::EventsDeleted    => write!(f, "events_deleted"),
            CalendarMetric::InvitesSent      => write!(f, "invites_sent"),
            CalendarMetric::InvitesAccepted  => write!(f, "invites_accepted"),
            CalendarMetric::InvitesDeclined  => write!(f, "invites_declined"),
            CalendarMetric::ActiveCalendars  => write!(f, "active_calendars"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarUsageParams {
    pub tenant_id: String,
    #[serde(default = "default_days")]
    pub days:      u32,
}

fn default_days() -> u32 {
    DEFAULT_DAYS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CalendarUsageRow {
    pub date:          String,
    pub metric:        String,
    pub value:         i64,
}

pub fn clamp_days(days: u32) -> u32 {
    days.max(1).min(MAX_DAYS)
}

pub fn acceptance_rate(accepted: u64, sent: u64) -> f64 {
    if sent == 0 { 0.0 } else { accepted as f64 / sent as f64 }
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
    fn metric_display_events_created() {
        assert_eq!(CalendarMetric::EventsCreated.to_string(), "events_created");
    }

    #[test]
    fn metric_display_events_deleted() {
        assert_eq!(CalendarMetric::EventsDeleted.to_string(), "events_deleted");
    }

    #[test]
    fn metric_display_invites_sent() {
        assert_eq!(CalendarMetric::InvitesSent.to_string(), "invites_sent");
    }

    #[test]
    fn metric_display_invites_accepted() {
        assert_eq!(CalendarMetric::InvitesAccepted.to_string(), "invites_accepted");
    }

    #[test]
    fn metric_display_invites_declined() {
        assert_eq!(CalendarMetric::InvitesDeclined.to_string(), "invites_declined");
    }

    #[test]
    fn metric_display_active_calendars() {
        assert_eq!(CalendarMetric::ActiveCalendars.to_string(), "active_calendars");
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
        assert_eq!(clamp_days(90), 90);
    }

    #[test]
    fn acceptance_rate_zero_sent() {
        assert_eq!(acceptance_rate(10, 0), 0.0);
    }

    #[test]
    fn acceptance_rate_half() {
        assert!((acceptance_rate(5, 10) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn acceptance_rate_full() {
        assert!((acceptance_rate(10, 10) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn acceptance_rate_zero_accepted() {
        assert_eq!(acceptance_rate(0, 100), 0.0);
    }

    #[test]
    fn default_days_fn_returns_30() {
        assert_eq!(default_days(), 30);
    }

    #[test]
    fn metric_equality() {
        assert_eq!(CalendarMetric::EventsCreated, CalendarMetric::EventsCreated);
        assert_ne!(CalendarMetric::EventsCreated, CalendarMetric::EventsDeleted);
    }

    #[test]
    fn serde_roundtrip_metric() {
        let m = CalendarMetric::InvitesSent;
        let json = serde_json::to_string(&m).unwrap();
        let back: CalendarMetric = serde_json::from_str(&json).unwrap();
        assert_eq!(back, CalendarMetric::InvitesSent);
    }

    #[test]
    fn calendar_usage_row_serde_roundtrip() {
        let row = CalendarUsageRow { date: "2026-05-01".into(), metric: "events_created".into(), value: 42 };
        let json = serde_json::to_string(&row).unwrap();
        let back: CalendarUsageRow = serde_json::from_str(&json).unwrap();
        assert_eq!(back, row);
    }

    #[test]
    fn clamp_days_at_max_boundary() {
        assert_eq!(clamp_days(MAX_DAYS), MAX_DAYS);
    }

    #[test]
    fn clamp_days_at_min_boundary() {
        assert_eq!(clamp_days(1), 1);
    }

    #[test]
    fn acceptance_rate_exceeds_one_when_invalid() {
        assert!(acceptance_rate(200, 100) > 1.0);
    }

    #[test]
    fn metric_clone_equals_original() {
        let m = CalendarMetric::InvitesAccepted;
        assert_eq!(m.clone(), m);
    }

    #[test]
    fn acceptance_rate_quarter() {
        assert!((acceptance_rate(25, 100) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn clamp_days_middle_value_unchanged() {
        assert_eq!(clamp_days(180), 180);
    }
}

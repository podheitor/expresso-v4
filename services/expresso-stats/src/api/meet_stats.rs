use serde::{Deserialize, Serialize};

pub const DEFAULT_DAYS: u32 = 30;
pub const MAX_DAYS: u32 = 365;
pub const SECS_PER_MINUTE: u64 = 60;
pub const SECS_PER_HOUR: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeetMetric {
    MeetingsHeld,
    TotalParticipants,
    TotalDurationSecs,
    RecordingsSaved,
    PeakConcurrent,
    ScreenShareMinutes,
}

impl std::fmt::Display for MeetMetric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeetMetric::MeetingsHeld       => write!(f, "meetings_held"),
            MeetMetric::TotalParticipants  => write!(f, "total_participants"),
            MeetMetric::TotalDurationSecs  => write!(f, "total_duration_secs"),
            MeetMetric::RecordingsSaved    => write!(f, "recordings_saved"),
            MeetMetric::PeakConcurrent     => write!(f, "peak_concurrent"),
            MeetMetric::ScreenShareMinutes => write!(f, "screen_share_minutes"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetStatsParams {
    pub tenant_id: String,
    #[serde(default = "default_days")]
    pub days:      u32,
}

fn default_days() -> u32 {
    DEFAULT_DAYS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MeetStatsRow {
    pub date:   String,
    pub metric: String,
    pub value:  i64,
}

pub fn secs_to_minutes(secs: u64) -> f64 {
    secs as f64 / SECS_PER_MINUTE as f64
}

pub fn secs_to_hours(secs: u64) -> f64 {
    secs as f64 / SECS_PER_HOUR as f64
}

pub fn avg_duration_secs(total_secs: u64, meetings: u64) -> f64 {
    if meetings == 0 { 0.0 } else { total_secs as f64 / meetings as f64 }
}

pub fn clamp_days(days: u32) -> u32 {
    days.max(1).min(MAX_DAYS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secs_per_minute_constant() {
        assert_eq!(SECS_PER_MINUTE, 60);
    }

    #[test]
    fn secs_per_hour_constant() {
        assert_eq!(SECS_PER_HOUR, 3600);
    }

    #[test]
    fn metric_display_meetings_held() {
        assert_eq!(MeetMetric::MeetingsHeld.to_string(), "meetings_held");
    }

    #[test]
    fn metric_display_total_participants() {
        assert_eq!(MeetMetric::TotalParticipants.to_string(), "total_participants");
    }

    #[test]
    fn metric_display_total_duration_secs() {
        assert_eq!(MeetMetric::TotalDurationSecs.to_string(), "total_duration_secs");
    }

    #[test]
    fn metric_display_recordings_saved() {
        assert_eq!(MeetMetric::RecordingsSaved.to_string(), "recordings_saved");
    }

    #[test]
    fn metric_display_peak_concurrent() {
        assert_eq!(MeetMetric::PeakConcurrent.to_string(), "peak_concurrent");
    }

    #[test]
    fn metric_display_screen_share_minutes() {
        assert_eq!(MeetMetric::ScreenShareMinutes.to_string(), "screen_share_minutes");
    }

    #[test]
    fn secs_to_minutes_one_hour() {
        assert!((secs_to_minutes(3600) - 60.0).abs() < 1e-9);
    }

    #[test]
    fn secs_to_hours_one_hour() {
        assert!((secs_to_hours(3600) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn secs_to_minutes_zero() {
        assert_eq!(secs_to_minutes(0), 0.0);
    }

    #[test]
    fn secs_to_hours_zero() {
        assert_eq!(secs_to_hours(0), 0.0);
    }

    #[test]
    fn avg_duration_zero_meetings() {
        assert_eq!(avg_duration_secs(1000, 0), 0.0);
    }

    #[test]
    fn avg_duration_computed() {
        assert!((avg_duration_secs(3600, 4) - 900.0).abs() < 1e-9);
    }

    #[test]
    fn avg_duration_zero_secs() {
        assert_eq!(avg_duration_secs(0, 10), 0.0);
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
        assert_eq!(clamp_days(14), 14);
    }

    #[test]
    fn default_days_fn_returns_30() {
        assert_eq!(default_days(), 30);
    }

    #[test]
    fn metric_equality() {
        assert_eq!(MeetMetric::MeetingsHeld, MeetMetric::MeetingsHeld);
        assert_ne!(MeetMetric::MeetingsHeld, MeetMetric::RecordingsSaved);
    }

    #[test]
    fn serde_roundtrip_metric() {
        let m = MeetMetric::PeakConcurrent;
        let json = serde_json::to_string(&m).unwrap();
        let back: MeetMetric = serde_json::from_str(&json).unwrap();
        assert_eq!(back, MeetMetric::PeakConcurrent);
    }

    #[test]
    fn meet_stats_row_serde_roundtrip() {
        let row = MeetStatsRow { date: "2026-05-01".into(), metric: "meetings_held".into(), value: 5 };
        let json = serde_json::to_string(&row).unwrap();
        let back: MeetStatsRow = serde_json::from_str(&json).unwrap();
        assert_eq!(back, row);
    }

    #[test]
    fn secs_to_hours_half_hour() {
        assert!((secs_to_hours(1800) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn default_days_constant() {
        assert_eq!(DEFAULT_DAYS, 30);
    }
}

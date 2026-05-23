use serde::{Deserialize, Serialize};

pub const DEFAULT_DAYS: u32 = 30;
pub const MAX_DAYS: u32 = 365;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatMetric {
    MessagesSent,
    MessagesReceived,
    ActiveRooms,
    NewRooms,
    FilesShared,
    ReactionsAdded,
}

impl std::fmt::Display for ChatMetric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatMetric::MessagesSent      => write!(f, "messages_sent"),
            ChatMetric::MessagesReceived  => write!(f, "messages_received"),
            ChatMetric::ActiveRooms       => write!(f, "active_rooms"),
            ChatMetric::NewRooms          => write!(f, "new_rooms"),
            ChatMetric::FilesShared       => write!(f, "files_shared"),
            ChatMetric::ReactionsAdded    => write!(f, "reactions_added"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatStatsParams {
    pub tenant_id: String,
    #[serde(default = "default_days")]
    pub days:      u32,
    pub room_id:   Option<String>,
}

fn default_days() -> u32 {
    DEFAULT_DAYS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatStatsRow {
    pub date:   String,
    pub metric: String,
    pub value:  i64,
}

pub fn engagement_ratio(messages: u64, active_users: u64) -> f64 {
    if active_users == 0 { 0.0 } else { messages as f64 / active_users as f64 }
}

pub fn clamp_days(days: u32) -> u32 {
    days.max(1).min(MAX_DAYS)
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
    fn metric_display_messages_sent() {
        assert_eq!(ChatMetric::MessagesSent.to_string(), "messages_sent");
    }

    #[test]
    fn metric_display_messages_received() {
        assert_eq!(ChatMetric::MessagesReceived.to_string(), "messages_received");
    }

    #[test]
    fn metric_display_active_rooms() {
        assert_eq!(ChatMetric::ActiveRooms.to_string(), "active_rooms");
    }

    #[test]
    fn metric_display_new_rooms() {
        assert_eq!(ChatMetric::NewRooms.to_string(), "new_rooms");
    }

    #[test]
    fn metric_display_files_shared() {
        assert_eq!(ChatMetric::FilesShared.to_string(), "files_shared");
    }

    #[test]
    fn metric_display_reactions_added() {
        assert_eq!(ChatMetric::ReactionsAdded.to_string(), "reactions_added");
    }

    #[test]
    fn clamp_days_below_min() {
        assert_eq!(clamp_days(0), 1);
    }

    #[test]
    fn clamp_days_above_max() {
        assert_eq!(clamp_days(MAX_DAYS + 100), MAX_DAYS);
    }

    #[test]
    fn clamp_days_within_range() {
        assert_eq!(clamp_days(60), 60);
    }

    #[test]
    fn engagement_ratio_zero_users() {
        assert_eq!(engagement_ratio(100, 0), 0.0);
    }

    #[test]
    fn engagement_ratio_computed() {
        assert!((engagement_ratio(100, 10) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn engagement_ratio_zero_messages() {
        assert_eq!(engagement_ratio(0, 50), 0.0);
    }

    #[test]
    fn engagement_ratio_fractional() {
        assert!((engagement_ratio(5, 10) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn default_days_fn_returns_30() {
        assert_eq!(default_days(), 30);
    }

    #[test]
    fn metric_equality() {
        assert_eq!(ChatMetric::MessagesSent, ChatMetric::MessagesSent);
        assert_ne!(ChatMetric::MessagesSent, ChatMetric::ActiveRooms);
    }

    #[test]
    fn serde_roundtrip_metric() {
        let m = ChatMetric::FilesShared;
        let json = serde_json::to_string(&m).unwrap();
        let back: ChatMetric = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ChatMetric::FilesShared);
    }

    #[test]
    fn chat_stats_row_serde_roundtrip() {
        let row = ChatStatsRow { date: "2026-05-01".into(), metric: "messages_sent".into(), value: 150 };
        let json = serde_json::to_string(&row).unwrap();
        let back: ChatStatsRow = serde_json::from_str(&json).unwrap();
        assert_eq!(back, row);
    }

    #[test]
    fn clamp_days_at_exact_max() {
        assert_eq!(clamp_days(365), 365);
    }

    #[test]
    fn clamp_days_at_exact_min() {
        assert_eq!(clamp_days(1), 1);
    }

    #[test]
    fn engagement_ratio_one_user_many_messages() {
        assert!((engagement_ratio(1000, 1) - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn metric_clone_equals_original() {
        let m = ChatMetric::ReactionsAdded;
        assert_eq!(m.clone(), m);
    }

    #[test]
    fn engagement_ratio_equal_messages_and_users() {
        assert!((engagement_ratio(10, 10) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn clamp_days_max_boundary_preserved() {
        assert_eq!(clamp_days(MAX_DAYS), MAX_DAYS);
    }
}

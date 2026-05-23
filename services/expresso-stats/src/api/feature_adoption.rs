use serde::{Deserialize, Serialize};

pub const DEFAULT_DAYS: u32 = 30;
pub const MAX_DAYS: u32 = 365;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Feature {
    Mail,
    Calendar,
    Drive,
    Meet,
    Chat,
    Contacts,
    Tasks,
    Notes,
}

impl std::fmt::Display for Feature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Feature::Mail      => write!(f, "mail"),
            Feature::Calendar  => write!(f, "calendar"),
            Feature::Drive     => write!(f, "drive"),
            Feature::Meet      => write!(f, "meet"),
            Feature::Chat      => write!(f, "chat"),
            Feature::Contacts  => write!(f, "contacts"),
            Feature::Tasks     => write!(f, "tasks"),
            Feature::Notes     => write!(f, "notes"),
        }
    }
}

impl Feature {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "mail"      => Some(Feature::Mail),
            "calendar"  => Some(Feature::Calendar),
            "drive"     => Some(Feature::Drive),
            "meet"      => Some(Feature::Meet),
            "chat"      => Some(Feature::Chat),
            "contacts"  => Some(Feature::Contacts),
            "tasks"     => Some(Feature::Tasks),
            "notes"     => Some(Feature::Notes),
            _           => None,
        }
    }

    pub fn all() -> &'static [Feature] {
        &[
            Feature::Mail, Feature::Calendar, Feature::Drive, Feature::Meet,
            Feature::Chat, Feature::Contacts, Feature::Tasks, Feature::Notes,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureAdoptionParams {
    pub tenant_id: String,
    #[serde(default = "default_days")]
    pub days:      u32,
    pub feature:   Option<String>,
}

fn default_days() -> u32 {
    DEFAULT_DAYS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeatureAdoptionRow {
    pub feature:      String,
    pub active_users: i64,
    pub total_users:  i64,
}

impl FeatureAdoptionRow {
    pub fn adoption_pct(&self) -> f64 {
        if self.total_users == 0 { 0.0 }
        else { self.active_users as f64 / self.total_users as f64 * 100.0 }
    }
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
    fn feature_display_mail() {
        assert_eq!(Feature::Mail.to_string(), "mail");
    }

    #[test]
    fn feature_display_calendar() {
        assert_eq!(Feature::Calendar.to_string(), "calendar");
    }

    #[test]
    fn feature_display_drive() {
        assert_eq!(Feature::Drive.to_string(), "drive");
    }

    #[test]
    fn feature_display_meet() {
        assert_eq!(Feature::Meet.to_string(), "meet");
    }

    #[test]
    fn feature_display_chat() {
        assert_eq!(Feature::Chat.to_string(), "chat");
    }

    #[test]
    fn feature_display_contacts() {
        assert_eq!(Feature::Contacts.to_string(), "contacts");
    }

    #[test]
    fn feature_display_tasks() {
        assert_eq!(Feature::Tasks.to_string(), "tasks");
    }

    #[test]
    fn feature_display_notes() {
        assert_eq!(Feature::Notes.to_string(), "notes");
    }

    #[test]
    fn from_str_mail() {
        assert_eq!(Feature::from_str("mail"), Some(Feature::Mail));
    }

    #[test]
    fn from_str_calendar_uppercase() {
        assert_eq!(Feature::from_str("CALENDAR"), Some(Feature::Calendar));
    }

    #[test]
    fn from_str_unknown_returns_none() {
        assert_eq!(Feature::from_str("video"), None);
    }

    #[test]
    fn all_features_count() {
        assert_eq!(Feature::all().len(), 8);
    }

    #[test]
    fn all_features_contains_mail() {
        assert!(Feature::all().contains(&Feature::Mail));
    }

    #[test]
    fn adoption_pct_zero_total() {
        let row = FeatureAdoptionRow { feature: "mail".into(), active_users: 0, total_users: 0 };
        assert_eq!(row.adoption_pct(), 0.0);
    }

    #[test]
    fn adoption_pct_half() {
        let row = FeatureAdoptionRow { feature: "mail".into(), active_users: 50, total_users: 100 };
        assert!((row.adoption_pct() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn adoption_pct_full() {
        let row = FeatureAdoptionRow { feature: "meet".into(), active_users: 100, total_users: 100 };
        assert!((row.adoption_pct() - 100.0).abs() < 1e-9);
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
        assert_eq!(clamp_days(30), 30);
    }

    #[test]
    fn default_days_fn_returns_30() {
        assert_eq!(default_days(), 30);
    }

    #[test]
    fn feature_equality() {
        assert_eq!(Feature::Mail, Feature::Mail);
        assert_ne!(Feature::Mail, Feature::Calendar);
    }

    #[test]
    fn serde_roundtrip_feature() {
        let f = Feature::Chat;
        let json = serde_json::to_string(&f).unwrap();
        let back: Feature = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Feature::Chat);
    }

    #[test]
    fn all_features_contains_notes() {
        assert!(Feature::all().contains(&Feature::Notes));
    }

    #[test]
    fn adoption_pct_quarter() {
        let row = FeatureAdoptionRow { feature: "drive".into(), active_users: 25, total_users: 100 };
        assert!((row.adoption_pct() - 25.0).abs() < 1e-9);
    }
}

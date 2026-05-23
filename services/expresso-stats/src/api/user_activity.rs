//! User activity analytics types and helpers — sprint #19857.

use serde::{Deserialize, Serialize};

/// Type of user activity event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityType {
    Login,
    EmailRead,
    EmailSent,
    FileUpload,
    FileDownload,
    CalendarEvent,
}

impl std::fmt::Display for ActivityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActivityType::Login         => write!(f, "login"),
            ActivityType::EmailRead     => write!(f, "email_read"),
            ActivityType::EmailSent     => write!(f, "email_sent"),
            ActivityType::FileUpload    => write!(f, "file_upload"),
            ActivityType::FileDownload  => write!(f, "file_download"),
            ActivityType::CalendarEvent => write!(f, "calendar_event"),
        }
    }
}

/// Aggregated user activity for one (bucket, activity_type) pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserActivityEntry {
    pub bucket:        String,
    pub activity_type: ActivityType,
    pub user_count:    u64,
    pub event_count:   u64,
}

/// Request parameters for user activity endpoint.
#[derive(Debug, Deserialize)]
pub struct UserActivityParams {
    pub tenant_id:     String,
    pub since_days:    Option<u32>,
    pub user_id:       Option<String>,
    pub activity_type: Option<ActivityType>,
}

pub const DEFAULT_SINCE_DAYS: u32 = 14;
pub const MAX_SINCE_DAYS:     u32 = 180;

pub fn validate_user_activity_params(p: &UserActivityParams) -> Option<String> {
    if p.tenant_id.trim().is_empty() {
        return Some("tenant_id must not be empty".into());
    }
    if let Some(d) = p.since_days {
        if d == 0 {
            return Some("since_days must be >= 1".into());
        }
        if d > MAX_SINCE_DAYS {
            return Some(format!("since_days must be <= {}", MAX_SINCE_DAYS));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_params(tenant: &str, since_days: Option<u32>) -> UserActivityParams {
        UserActivityParams {
            tenant_id: tenant.into(), since_days,
            user_id: None, activity_type: None,
        }
    }

    #[test]
    fn activity_type_login_display() {
        assert_eq!(ActivityType::Login.to_string(), "login");
    }

    #[test]
    fn activity_type_email_read_display() {
        assert_eq!(ActivityType::EmailRead.to_string(), "email_read");
    }

    #[test]
    fn activity_type_email_sent_display() {
        assert_eq!(ActivityType::EmailSent.to_string(), "email_sent");
    }

    #[test]
    fn activity_type_file_upload_display() {
        assert_eq!(ActivityType::FileUpload.to_string(), "file_upload");
    }

    #[test]
    fn activity_type_file_download_display() {
        assert_eq!(ActivityType::FileDownload.to_string(), "file_download");
    }

    #[test]
    fn activity_type_calendar_event_display() {
        assert_eq!(ActivityType::CalendarEvent.to_string(), "calendar_event");
    }

    #[test]
    fn activity_type_eq_same_variant() {
        assert_eq!(ActivityType::Login, ActivityType::Login);
    }

    #[test]
    fn activity_type_ne_different_variants() {
        assert_ne!(ActivityType::Login, ActivityType::EmailRead);
    }

    #[test]
    fn default_since_days_is_14() {
        assert_eq!(DEFAULT_SINCE_DAYS, 14);
    }

    #[test]
    fn max_since_days_is_180() {
        assert_eq!(MAX_SINCE_DAYS, 180);
    }

    #[test]
    fn validate_empty_tenant_returns_error() {
        assert!(validate_user_activity_params(&make_params("", None)).is_some());
    }

    #[test]
    fn validate_valid_params_returns_none() {
        assert!(validate_user_activity_params(&make_params("t1", Some(7))).is_none());
    }

    #[test]
    fn validate_since_days_zero_returns_error() {
        assert!(validate_user_activity_params(&make_params("t1", Some(0))).is_some());
    }

    #[test]
    fn validate_since_days_above_max_returns_error() {
        assert!(validate_user_activity_params(&make_params("t1", Some(MAX_SINCE_DAYS + 1))).is_some());
    }

    #[test]
    fn validate_since_days_at_max_is_ok() {
        assert!(validate_user_activity_params(&make_params("t1", Some(MAX_SINCE_DAYS))).is_none());
    }

    #[test]
    fn user_activity_entry_fields_accessible() {
        let e = UserActivityEntry {
            bucket: "2026-05-01".into(),
            activity_type: ActivityType::Login,
            user_count: 50,
            event_count: 120,
        };
        assert_eq!(e.user_count, 50);
        assert_eq!(e.event_count, 120);
    }

    #[test]
    fn activity_type_serde_roundtrip_login() {
        let json = serde_json::to_string(&ActivityType::Login).unwrap();
        let back: ActivityType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ActivityType::Login);
    }

    #[test]
    fn activity_type_serde_roundtrip_file_upload() {
        let json = serde_json::to_string(&ActivityType::FileUpload).unwrap();
        let back: ActivityType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ActivityType::FileUpload);
    }

    #[test]
    fn default_less_than_max_since_days() {
        assert!(DEFAULT_SINCE_DAYS < MAX_SINCE_DAYS);
    }

    #[test]
    fn validate_whitespace_tenant_returns_error() {
        assert!(validate_user_activity_params(&make_params("  ", None)).is_some());
    }

    #[test]
    fn validate_no_since_days_is_ok() {
        assert!(validate_user_activity_params(&make_params("t1", None)).is_none());
    }

    #[test]
    fn all_activity_type_displays_nonempty() {
        for a in [
            ActivityType::Login, ActivityType::EmailRead, ActivityType::EmailSent,
            ActivityType::FileUpload, ActivityType::FileDownload, ActivityType::CalendarEvent,
        ] {
            assert!(!a.to_string().is_empty());
        }
    }

    #[test]
    fn activity_type_serde_roundtrip_calendar_event() {
        let json = serde_json::to_string(&ActivityType::CalendarEvent).unwrap();
        let back: ActivityType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ActivityType::CalendarEvent);
    }

    #[test]
    fn user_activity_entry_bucket_field() {
        let e = UserActivityEntry {
            bucket: "2026-05-22".into(),
            activity_type: ActivityType::EmailSent,
            user_count: 5,
            event_count: 20,
        };
        assert_eq!(e.bucket, "2026-05-22");
    }

    #[test]
    fn activity_type_serde_roundtrip_file_download() {
        let json = serde_json::to_string(&ActivityType::FileDownload).unwrap();
        let back: ActivityType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ActivityType::FileDownload);
    }

    #[test]
    fn activity_type_serde_roundtrip_email_sent() {
        let json = serde_json::to_string(&ActivityType::EmailSent).unwrap();
        let back: ActivityType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ActivityType::EmailSent);
    }
}

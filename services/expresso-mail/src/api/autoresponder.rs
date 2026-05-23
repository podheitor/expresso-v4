//! Auto-responder settings — per-user automatic reply.
//!
//! GET  /api/v1/mail/autoresponder — get current settings
//! PUT  /api/v1/mail/autoresponder — update settings

use serde::{Deserialize, Serialize};

pub const MAX_SUBJECT_BYTES: usize = 998;
pub const MAX_BODY_BYTES:    usize = 8 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutoresponderSettings {
    pub enabled:  bool,
    pub subject:  String,
    pub body:     String,
    /// Optional interval in days between replies to the same sender.
    pub interval_days: Option<u32>,
}

impl Default for AutoresponderSettings {
    fn default() -> Self {
        Self {
            enabled:       false,
            subject:       String::new(),
            body:          String::new(),
            interval_days: Some(1),
        }
    }
}

pub fn validate_settings(s: &AutoresponderSettings) -> Result<(), &'static str> {
    if s.subject.len() > MAX_SUBJECT_BYTES {
        return Err("subject too long");
    }
    if s.body.len() > MAX_BODY_BYTES {
        return Err("body too long");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled() {
        assert!(!AutoresponderSettings::default().enabled);
    }

    #[test]
    fn default_subject_empty() {
        assert!(AutoresponderSettings::default().subject.is_empty());
    }

    #[test]
    fn default_body_empty() {
        assert!(AutoresponderSettings::default().body.is_empty());
    }

    #[test]
    fn default_interval_days_is_one() {
        assert_eq!(AutoresponderSettings::default().interval_days, Some(1));
    }

    #[test]
    fn max_subject_bytes_constant() {
        assert_eq!(MAX_SUBJECT_BYTES, 998);
    }

    #[test]
    fn max_body_bytes_constant() {
        assert_eq!(MAX_BODY_BYTES, 8 * 1024);
    }

    #[test]
    fn validate_empty_settings_ok() {
        let s = AutoresponderSettings::default();
        assert!(validate_settings(&s).is_ok());
    }

    #[test]
    fn validate_subject_too_long_err() {
        let s = AutoresponderSettings {
            subject: "x".repeat(MAX_SUBJECT_BYTES + 1),
            ..AutoresponderSettings::default()
        };
        assert!(validate_settings(&s).is_err());
    }

    #[test]
    fn validate_body_too_long_err() {
        let s = AutoresponderSettings {
            body: "x".repeat(MAX_BODY_BYTES + 1),
            ..AutoresponderSettings::default()
        };
        assert!(validate_settings(&s).is_err());
    }

    #[test]
    fn serde_roundtrip() {
        let s = AutoresponderSettings {
            enabled: true,
            subject: "Out of office".into(),
            body:    "I am away".into(),
            interval_days: Some(3),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: AutoresponderSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn interval_days_none_roundtrip() {
        let s = AutoresponderSettings {
            interval_days: None,
            ..AutoresponderSettings::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: AutoresponderSettings = serde_json::from_str(&json).unwrap();
        assert!(back.interval_days.is_none());
    }

    #[test]
    fn clone_preserves_enabled_flag() {
        let s = AutoresponderSettings { enabled: true, ..AutoresponderSettings::default() };
        assert!(s.clone().enabled);
    }

    #[test]
    fn equality_check() {
        let a = AutoresponderSettings::default();
        let b = AutoresponderSettings::default();
        assert_eq!(a, b);
    }

    #[test]
    fn subject_at_max_length_is_valid() {
        let s = AutoresponderSettings {
            subject: "x".repeat(MAX_SUBJECT_BYTES),
            ..AutoresponderSettings::default()
        };
        assert!(validate_settings(&s).is_ok());
    }

    #[test]
    fn body_at_max_length_is_valid() {
        let s = AutoresponderSettings {
            body: "x".repeat(MAX_BODY_BYTES),
            ..AutoresponderSettings::default()
        };
        assert!(validate_settings(&s).is_ok());
    }

    #[test]
    fn enabled_true_field_preserved() {
        let s = AutoresponderSettings {
            enabled: true, subject: "Hi".into(), body: "Away".into(),
            interval_days: None,
        };
        assert!(s.enabled);
        assert_eq!(s.subject, "Hi");
    }

    #[test]
    fn serde_json_contains_enabled_key() {
        let s = AutoresponderSettings::default();
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("enabled"));
    }

    #[test]
    fn serde_json_contains_subject_key() {
        let s = AutoresponderSettings::default();
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("subject"));
    }

    #[test]
    fn interval_days_zero_roundtrip() {
        let s = AutoresponderSettings {
            interval_days: Some(0),
            ..AutoresponderSettings::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: AutoresponderSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.interval_days, Some(0));
    }

    #[test]
    fn validate_both_at_max_is_ok() {
        let s = AutoresponderSettings {
            subject: "x".repeat(MAX_SUBJECT_BYTES),
            body:    "y".repeat(MAX_BODY_BYTES),
            ..AutoresponderSettings::default()
        };
        assert!(validate_settings(&s).is_ok());
    }

    #[test]
    fn max_body_is_eight_kib() {
        assert_eq!(MAX_BODY_BYTES, 8192);
    }

    #[test]
    fn subject_error_message_is_subject_too_long() {
        let s = AutoresponderSettings {
            subject: "x".repeat(MAX_SUBJECT_BYTES + 1),
            ..AutoresponderSettings::default()
        };
        assert_eq!(validate_settings(&s), Err("subject too long"));
    }

    #[test]
    fn body_error_message_is_body_too_long() {
        let s = AutoresponderSettings {
            body: "x".repeat(MAX_BODY_BYTES + 1),
            ..AutoresponderSettings::default()
        };
        assert_eq!(validate_settings(&s), Err("body too long"));
    }

    #[test]
    fn max_subject_bytes_is_998() {
        assert_eq!(MAX_SUBJECT_BYTES, 998);
    }

    #[test]
    fn default_settings_ne_enabled_variant() {
        let mut s = AutoresponderSettings::default();
        s.enabled = true;
        assert_ne!(s, AutoresponderSettings::default());
    }

    #[test]
    fn max_subject_less_than_max_body() {
        assert!(MAX_SUBJECT_BYTES < MAX_BODY_BYTES * 10);
        assert!(MAX_BODY_BYTES > MAX_SUBJECT_BYTES);
    }
}

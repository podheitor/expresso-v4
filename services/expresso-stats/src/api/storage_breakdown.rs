//! Storage breakdown analytics types and helpers — sprint #19857.

use serde::{Deserialize, Serialize};

/// Storage category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageCategory {
    Mail,
    Attachments,
    Drive,
    Calendar,
    Contacts,
    Other,
}

impl std::fmt::Display for StorageCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageCategory::Mail        => write!(f, "mail"),
            StorageCategory::Attachments => write!(f, "attachments"),
            StorageCategory::Drive       => write!(f, "drive"),
            StorageCategory::Calendar    => write!(f, "calendar"),
            StorageCategory::Contacts    => write!(f, "contacts"),
            StorageCategory::Other       => write!(f, "other"),
        }
    }
}

/// Storage usage for one category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageEntry {
    pub category:   StorageCategory,
    pub bytes:      u64,
    pub pct:        f64,
}

/// Request parameters for storage breakdown endpoint.
#[derive(Debug, Deserialize)]
pub struct StorageBreakdownParams {
    pub tenant_id: String,
    pub user_id:   Option<String>,
}

pub fn validate_storage_params(p: &StorageBreakdownParams) -> Option<String> {
    if p.tenant_id.trim().is_empty() {
        return Some("tenant_id must not be empty".into());
    }
    None
}

/// Format bytes as a human-readable string (B / KiB / MiB / GiB).
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_category_mail_display() {
        assert_eq!(StorageCategory::Mail.to_string(), "mail");
    }

    #[test]
    fn storage_category_attachments_display() {
        assert_eq!(StorageCategory::Attachments.to_string(), "attachments");
    }

    #[test]
    fn storage_category_drive_display() {
        assert_eq!(StorageCategory::Drive.to_string(), "drive");
    }

    #[test]
    fn storage_category_calendar_display() {
        assert_eq!(StorageCategory::Calendar.to_string(), "calendar");
    }

    #[test]
    fn storage_category_contacts_display() {
        assert_eq!(StorageCategory::Contacts.to_string(), "contacts");
    }

    #[test]
    fn storage_category_other_display() {
        assert_eq!(StorageCategory::Other.to_string(), "other");
    }

    #[test]
    fn storage_category_eq_same() {
        assert_eq!(StorageCategory::Mail, StorageCategory::Mail);
    }

    #[test]
    fn storage_category_ne_different() {
        assert_ne!(StorageCategory::Mail, StorageCategory::Drive);
    }

    #[test]
    fn format_bytes_less_than_1kib() {
        assert_eq!(format_bytes(512), "512 B");
    }

    #[test]
    fn format_bytes_exactly_1kib() {
        assert_eq!(format_bytes(1024), "1.0 KiB");
    }

    #[test]
    fn format_bytes_mib() {
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn format_bytes_gib() {
        let s = format_bytes(1024 * 1024 * 1024);
        assert!(s.contains("GiB"));
    }

    #[test]
    fn format_bytes_zero() {
        assert_eq!(format_bytes(0), "0 B");
    }

    #[test]
    fn validate_empty_tenant_returns_error() {
        let p = StorageBreakdownParams { tenant_id: "".into(), user_id: None };
        assert!(validate_storage_params(&p).is_some());
    }

    #[test]
    fn validate_valid_params_returns_none() {
        let p = StorageBreakdownParams { tenant_id: "t1".into(), user_id: None };
        assert!(validate_storage_params(&p).is_none());
    }

    #[test]
    fn validate_whitespace_tenant_returns_error() {
        let p = StorageBreakdownParams { tenant_id: "  ".into(), user_id: None };
        assert!(validate_storage_params(&p).is_some());
    }

    #[test]
    fn storage_entry_fields_accessible() {
        let e = StorageEntry { category: StorageCategory::Mail, bytes: 1_000_000, pct: 40.0 };
        assert_eq!(e.bytes, 1_000_000);
        assert!((e.pct - 40.0).abs() < 1e-9);
    }

    #[test]
    fn storage_category_serde_roundtrip_drive() {
        let json = serde_json::to_string(&StorageCategory::Drive).unwrap();
        let back: StorageCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(back, StorageCategory::Drive);
    }

    #[test]
    fn storage_category_serde_roundtrip_other() {
        let json = serde_json::to_string(&StorageCategory::Other).unwrap();
        let back: StorageCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(back, StorageCategory::Other);
    }

    #[test]
    fn format_bytes_1023_is_bytes() {
        let s = format_bytes(1023);
        assert!(s.ends_with("B"));
        assert!(!s.contains("KiB"));
    }

    #[test]
    fn format_bytes_2kib() {
        let s = format_bytes(2048);
        assert!(s.contains("KiB"));
    }

    #[test]
    fn all_category_displays_are_nonempty() {
        for c in [
            StorageCategory::Mail, StorageCategory::Attachments, StorageCategory::Drive,
            StorageCategory::Calendar, StorageCategory::Contacts, StorageCategory::Other,
        ] {
            assert!(!c.to_string().is_empty());
        }
    }

    #[test]
    fn storage_category_serde_roundtrip_attachments() {
        let json = serde_json::to_string(&StorageCategory::Attachments).unwrap();
        let back: StorageCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(back, StorageCategory::Attachments);
    }

    #[test]
    fn validate_with_user_id_is_ok() {
        let p = StorageBreakdownParams { tenant_id: "t1".into(), user_id: Some("bob".into()) };
        assert!(validate_storage_params(&p).is_none());
    }

    #[test]
    fn storage_category_serde_roundtrip_contacts() {
        let json = serde_json::to_string(&StorageCategory::Contacts).unwrap();
        let back: StorageCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(back, StorageCategory::Contacts);
    }

    #[test]
    fn format_bytes_exactly_1_mib_contains_mib_unit() {
        let s = format_bytes(1024 * 1024);
        assert!(s.contains("MiB"));
    }
}

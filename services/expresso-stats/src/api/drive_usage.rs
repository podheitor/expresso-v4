use serde::{Deserialize, Serialize};

pub const DEFAULT_DAYS: u32 = 30;
pub const MAX_DAYS: u32 = 365;
pub const BYTES_PER_KIB: u64 = 1024;
pub const BYTES_PER_MIB: u64 = 1024 * 1024;
pub const BYTES_PER_GIB: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriveMetric {
    StorageUsedBytes,
    FilesUploaded,
    FilesDownloaded,
    FilesDeleted,
    FilesShared,
    ActiveUsers,
}

impl std::fmt::Display for DriveMetric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriveMetric::StorageUsedBytes => write!(f, "storage_used_bytes"),
            DriveMetric::FilesUploaded    => write!(f, "files_uploaded"),
            DriveMetric::FilesDownloaded  => write!(f, "files_downloaded"),
            DriveMetric::FilesDeleted     => write!(f, "files_deleted"),
            DriveMetric::FilesShared      => write!(f, "files_shared"),
            DriveMetric::ActiveUsers      => write!(f, "active_users"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveUsageParams {
    pub tenant_id: String,
    #[serde(default = "default_days")]
    pub days:      u32,
}

fn default_days() -> u32 {
    DEFAULT_DAYS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DriveUsageRow {
    pub date:   String,
    pub metric: String,
    pub value:  i64,
}

pub fn bytes_to_gib(bytes: u64) -> f64 {
    bytes as f64 / BYTES_PER_GIB as f64
}

pub fn bytes_to_mib(bytes: u64) -> f64 {
    bytes as f64 / BYTES_PER_MIB as f64
}

pub fn clamp_days(days: u32) -> u32 {
    days.max(1).min(MAX_DAYS)
}

pub fn quota_pct(used: u64, quota: u64) -> f64 {
    if quota == 0 { 0.0 } else { (used as f64 / quota as f64) * 100.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_per_kib_constant() {
        assert_eq!(BYTES_PER_KIB, 1024);
    }

    #[test]
    fn bytes_per_mib_constant() {
        assert_eq!(BYTES_PER_MIB, 1024 * 1024);
    }

    #[test]
    fn bytes_per_gib_constant() {
        assert_eq!(BYTES_PER_GIB, 1024 * 1024 * 1024);
    }

    #[test]
    fn metric_display_storage_used_bytes() {
        assert_eq!(DriveMetric::StorageUsedBytes.to_string(), "storage_used_bytes");
    }

    #[test]
    fn metric_display_files_uploaded() {
        assert_eq!(DriveMetric::FilesUploaded.to_string(), "files_uploaded");
    }

    #[test]
    fn metric_display_files_downloaded() {
        assert_eq!(DriveMetric::FilesDownloaded.to_string(), "files_downloaded");
    }

    #[test]
    fn metric_display_files_deleted() {
        assert_eq!(DriveMetric::FilesDeleted.to_string(), "files_deleted");
    }

    #[test]
    fn metric_display_files_shared() {
        assert_eq!(DriveMetric::FilesShared.to_string(), "files_shared");
    }

    #[test]
    fn metric_display_active_users() {
        assert_eq!(DriveMetric::ActiveUsers.to_string(), "active_users");
    }

    #[test]
    fn bytes_to_gib_one_gib() {
        assert!((bytes_to_gib(BYTES_PER_GIB) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn bytes_to_mib_one_mib() {
        assert!((bytes_to_mib(BYTES_PER_MIB) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn bytes_to_gib_zero() {
        assert_eq!(bytes_to_gib(0), 0.0);
    }

    #[test]
    fn bytes_to_mib_zero() {
        assert_eq!(bytes_to_mib(0), 0.0);
    }

    #[test]
    fn quota_pct_zero_quota() {
        assert_eq!(quota_pct(500, 0), 0.0);
    }

    #[test]
    fn quota_pct_half() {
        assert!((quota_pct(500, 1000) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn quota_pct_full() {
        assert!((quota_pct(1000, 1000) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn clamp_days_below_min() {
        assert_eq!(clamp_days(0), 1);
    }

    #[test]
    fn clamp_days_above_max() {
        assert_eq!(clamp_days(MAX_DAYS + 1), MAX_DAYS);
    }

    #[test]
    fn clamp_days_within_range() {
        assert_eq!(clamp_days(90), 90);
    }

    #[test]
    fn default_days_fn_returns_30() {
        assert_eq!(default_days(), 30);
    }

    #[test]
    fn metric_equality() {
        assert_eq!(DriveMetric::FilesUploaded, DriveMetric::FilesUploaded);
        assert_ne!(DriveMetric::FilesUploaded, DriveMetric::FilesDeleted);
    }

    #[test]
    fn serde_roundtrip_metric() {
        let m = DriveMetric::FilesShared;
        let json = serde_json::to_string(&m).unwrap();
        let back: DriveMetric = serde_json::from_str(&json).unwrap();
        assert_eq!(back, DriveMetric::FilesShared);
    }

    #[test]
    fn drive_usage_row_serde_roundtrip() {
        let row = DriveUsageRow { date: "2026-05-01".into(), metric: "files_uploaded".into(), value: 77 };
        let json = serde_json::to_string(&row).unwrap();
        let back: DriveUsageRow = serde_json::from_str(&json).unwrap();
        assert_eq!(back, row);
    }

    #[test]
    fn bytes_to_gib_two_gib() {
        assert!((bytes_to_gib(2 * BYTES_PER_GIB) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn default_days_constant() {
        assert_eq!(DEFAULT_DAYS, 30);
    }

    #[test]
    fn quota_pct_zero_used() {
        assert_eq!(quota_pct(0, 1000), 0.0);
    }
}

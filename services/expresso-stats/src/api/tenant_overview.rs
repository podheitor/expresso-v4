use serde::{Deserialize, Serialize};

pub const DEFAULT_DAYS: u32 = 30;
pub const MAX_DAYS: u32 = 365;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverviewMetric {
    TotalUsers,
    ActiveUsers,
    StorageUsedBytes,
    EmailsSent,
    MeetingsHeld,
    DriveFiles,
}

impl std::fmt::Display for OverviewMetric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OverviewMetric::TotalUsers        => write!(f, "total_users"),
            OverviewMetric::ActiveUsers       => write!(f, "active_users"),
            OverviewMetric::StorageUsedBytes  => write!(f, "storage_used_bytes"),
            OverviewMetric::EmailsSent        => write!(f, "emails_sent"),
            OverviewMetric::MeetingsHeld      => write!(f, "meetings_held"),
            OverviewMetric::DriveFiles        => write!(f, "drive_files"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TenantOverview {
    pub tenant_id:           String,
    pub total_users:         i64,
    pub active_users:        i64,
    pub storage_used_bytes:  i64,
    pub emails_sent:         i64,
    pub meetings_held:       i64,
    pub drive_files:         i64,
}

impl TenantOverview {
    pub fn active_user_pct(&self) -> f64 {
        if self.total_users == 0 {
            0.0
        } else {
            self.active_users as f64 / self.total_users as f64 * 100.0
        }
    }

    pub fn storage_gib(&self) -> f64 {
        self.storage_used_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantOverviewParams {
    pub tenant_id: String,
    #[serde(default = "default_days")]
    pub days:      u32,
}

fn default_days() -> u32 {
    DEFAULT_DAYS
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
    fn metric_display_total_users() {
        assert_eq!(OverviewMetric::TotalUsers.to_string(), "total_users");
    }

    #[test]
    fn metric_display_active_users() {
        assert_eq!(OverviewMetric::ActiveUsers.to_string(), "active_users");
    }

    #[test]
    fn metric_display_storage_used_bytes() {
        assert_eq!(OverviewMetric::StorageUsedBytes.to_string(), "storage_used_bytes");
    }

    #[test]
    fn metric_display_emails_sent() {
        assert_eq!(OverviewMetric::EmailsSent.to_string(), "emails_sent");
    }

    #[test]
    fn metric_display_meetings_held() {
        assert_eq!(OverviewMetric::MeetingsHeld.to_string(), "meetings_held");
    }

    #[test]
    fn metric_display_drive_files() {
        assert_eq!(OverviewMetric::DriveFiles.to_string(), "drive_files");
    }

    #[test]
    fn active_user_pct_zero_total() {
        let o = TenantOverview { tenant_id: "t".into(), total_users: 0, active_users: 0,
            storage_used_bytes: 0, emails_sent: 0, meetings_held: 0, drive_files: 0 };
        assert_eq!(o.active_user_pct(), 0.0);
    }

    #[test]
    fn active_user_pct_half() {
        let o = TenantOverview { tenant_id: "t".into(), total_users: 100, active_users: 50,
            storage_used_bytes: 0, emails_sent: 0, meetings_held: 0, drive_files: 0 };
        assert!((o.active_user_pct() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn active_user_pct_full() {
        let o = TenantOverview { tenant_id: "t".into(), total_users: 10, active_users: 10,
            storage_used_bytes: 0, emails_sent: 0, meetings_held: 0, drive_files: 0 };
        assert!((o.active_user_pct() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn storage_gib_computed() {
        let gib = 1024i64 * 1024 * 1024;
        let o = TenantOverview { tenant_id: "t".into(), total_users: 0, active_users: 0,
            storage_used_bytes: gib, emails_sent: 0, meetings_held: 0, drive_files: 0 };
        assert!((o.storage_gib() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn storage_gib_zero() {
        let o = TenantOverview { tenant_id: "t".into(), total_users: 0, active_users: 0,
            storage_used_bytes: 0, emails_sent: 0, meetings_held: 0, drive_files: 0 };
        assert_eq!(o.storage_gib(), 0.0);
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
        assert_eq!(clamp_days(30), 30);
    }

    #[test]
    fn default_days_fn_returns_30() {
        assert_eq!(default_days(), 30);
    }

    #[test]
    fn metric_equality() {
        assert_eq!(OverviewMetric::TotalUsers, OverviewMetric::TotalUsers);
        assert_ne!(OverviewMetric::TotalUsers, OverviewMetric::ActiveUsers);
    }

    #[test]
    fn serde_roundtrip_metric() {
        let m = OverviewMetric::DriveFiles;
        let json = serde_json::to_string(&m).unwrap();
        let back: OverviewMetric = serde_json::from_str(&json).unwrap();
        assert_eq!(back, OverviewMetric::DriveFiles);
    }

    #[test]
    fn tenant_overview_serde_roundtrip() {
        let o = TenantOverview { tenant_id: "t1".into(), total_users: 100, active_users: 80,
            storage_used_bytes: 1024, emails_sent: 500, meetings_held: 10, drive_files: 200 };
        let json = serde_json::to_string(&o).unwrap();
        let back: TenantOverview = serde_json::from_str(&json).unwrap();
        assert_eq!(back, o);
    }

    #[test]
    fn active_user_pct_quarter() {
        let o = TenantOverview { tenant_id: "t".into(), total_users: 400, active_users: 100,
            storage_used_bytes: 0, emails_sent: 0, meetings_held: 0, drive_files: 0 };
        assert!((o.active_user_pct() - 25.0).abs() < 1e-9);
    }

    #[test]
    fn max_days_is_365() {
        assert_eq!(MAX_DAYS, 365);
    }

    #[test]
    fn storage_gib_half_gib() {
        let half_gib = (1024i64 * 1024 * 1024) / 2;
        let o = TenantOverview { tenant_id: "t".into(), total_users: 0, active_users: 0,
            storage_used_bytes: half_gib, emails_sent: 0, meetings_held: 0, drive_files: 0 };
        assert!((o.storage_gib() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn active_user_pct_zero_active_users() {
        let o = TenantOverview { tenant_id: "t".into(), total_users: 100, active_users: 0,
            storage_used_bytes: 0, emails_sent: 0, meetings_held: 0, drive_files: 0 };
        assert_eq!(o.active_user_pct(), 0.0);
    }
}

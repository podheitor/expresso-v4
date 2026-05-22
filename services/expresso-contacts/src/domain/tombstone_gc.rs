//! Background GC → purge expired CardDAV contact tombstones.
//!
//! RFC 6578 sync-collection: retention window (default 30 days) keeps
//! tombstones available for offline clients. After expiry, rows are deleted.

use std::time::Duration;

use expresso_core::DbPool;
use tokio::time::interval;
use tracing::{info, warn};

pub const DEFAULT_RETENTION_DAYS: i32 = 30;
pub const DEFAULT_INTERVAL_HOURS: u64 = 6;

pub fn spawn(pool: DbPool, retention_days: i32, interval_hours: u64) {
    let hours = interval_hours.max(1);
    let days = retention_days.max(1);
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(hours * 3600));
        loop {
            tick.tick().await;
            match purge_once(&pool, days).await {
                Ok(n) => info!(deleted = n, retention_days = days, "tombstone GC cycle completed"),
                Err(e) => warn!(error = %e, "tombstone GC failed"),
            }
        }
    });
}

pub async fn purge_once(pool: &DbPool, retention_days: i32) -> sqlx::Result<u64> {
    let res = sqlx::query(
        "DELETE FROM contact_tombstones \
         WHERE deleted_at < now() - make_interval(days => $1::int)",
    )
    .bind(retention_days)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_reasonable() {
        assert!(DEFAULT_RETENTION_DAYS >= 7);
        assert!(DEFAULT_INTERVAL_HOURS >= 1);
    }

    #[test]
    fn default_retention_is_30_days() {
        assert_eq!(DEFAULT_RETENTION_DAYS, 30);
    }

    #[test]
    fn default_interval_is_6_hours() {
        assert_eq!(DEFAULT_INTERVAL_HOURS, 6);
    }

    #[test]
    fn retention_days_positive() {
        assert!(DEFAULT_RETENTION_DAYS > 0);
    }

    #[test]
    fn interval_hours_at_least_one() {
        assert!(DEFAULT_INTERVAL_HOURS >= 1);
    }

    #[test]
    fn retention_days_under_year() {
        assert!(DEFAULT_RETENTION_DAYS < 365);
    }

    #[test]
    fn interval_hours_under_day() {
        assert!(DEFAULT_INTERVAL_HOURS <= 24);
    }

    #[test]
    fn retention_days_at_least_seven() {
        assert!(DEFAULT_RETENTION_DAYS >= 7);
    }

    #[test]
    fn interval_and_retention_constants_positive() {
        assert!(DEFAULT_INTERVAL_HOURS > 0);
        assert!(DEFAULT_RETENTION_DAYS > 0);
    }

    #[test]
    fn retention_days_is_thirty() {
        assert_eq!(DEFAULT_RETENTION_DAYS, 30);
    }

    #[test]
    fn interval_hours_is_positive() {
        assert!(DEFAULT_INTERVAL_HOURS > 0);
    }

    #[test]
    fn default_interval_is_six_hours() {
        assert_eq!(DEFAULT_INTERVAL_HOURS, 6);
    }

    #[test]
    fn retention_days_default_is_thirty() {
        assert_eq!(DEFAULT_RETENTION_DAYS, 30);
    }

    #[test]
    fn interval_hours_default_is_six() {
        assert_eq!(DEFAULT_INTERVAL_HOURS, 6);
    }

    #[test]
    fn retention_days_in_hours_exceeds_interval() {
        assert!(DEFAULT_RETENTION_DAYS as u64 * 24 > DEFAULT_INTERVAL_HOURS);
    }

    #[test]
    fn retention_constant_type_is_i32() {
        // Ensures DEFAULT_RETENTION_DAYS fits in the sqlx BIND type (i32).
        let _: i32 = DEFAULT_RETENTION_DAYS;
        assert!(DEFAULT_RETENTION_DAYS > 0);
    }
}

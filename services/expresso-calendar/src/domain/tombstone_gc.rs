//! Background GC → purge expired CalDAV event tombstones.
//!
//! RFC 6578 sync-collection: clients compare their last sync-token with
//! collection ctag; tombstones carry `deleted_ctag` + `deleted_at`. We retain
//! each tombstone for a configurable window (default 30 days) so offline
//! clients can still catch up. After that, rows are deleted — clients that
//! were offline longer than the window and still hold an older token will
//! simply miss those deletions (acceptable trade-off per RFC 6578 §3.8).

use std::time::Duration;

use expresso_core::DbPool;
use tokio::time::interval;
use tracing::{info, warn};

/// Default retention window = 30 days.
pub const DEFAULT_RETENTION_DAYS: i32 = 30;
/// Default GC cycle cadence = 6 hours.
pub const DEFAULT_INTERVAL_HOURS: u64 = 6;

/// Spawn the background GC task. Non-blocking.
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

/// Single purge cycle — deletes tombstone rows older than `retention_days`.
/// Returns rows affected.
pub async fn purge_once(pool: &DbPool, retention_days: i32) -> sqlx::Result<u64> {
    let res = sqlx::query(
        "DELETE FROM calendar_event_tombstones \
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
    fn retention_days_less_than_365() {
        assert!(DEFAULT_RETENTION_DAYS < 365);
    }

    #[test]
    fn interval_hours_is_reasonable() {
        assert!(DEFAULT_INTERVAL_HOURS <= 24);
    }

    #[test]
    fn retention_days_at_least_seven() {
        assert!(DEFAULT_RETENTION_DAYS >= 7);
    }

    #[test]
    fn retention_greater_than_interval_hours() {
        assert!(DEFAULT_RETENTION_DAYS as u64 * 24 > DEFAULT_INTERVAL_HOURS);
    }

    #[test]
    fn interval_hours_is_positive() {
        assert!(DEFAULT_INTERVAL_HOURS > 0);
    }

    #[test]
    fn retention_days_is_positive() {
        assert!(DEFAULT_RETENTION_DAYS > 0);
    }

    #[test]
    fn retention_days_exceeds_one_week() {
        assert!(DEFAULT_RETENTION_DAYS > 7);
    }
}

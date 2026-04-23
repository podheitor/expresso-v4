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
}

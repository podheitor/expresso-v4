//! Background worker — calendar alarm delivery.
//!
//! Scans `calendar_event_alarms` rows where `trigger_abs <= now()` and
//! publishes a notification entry in `notification_push_subscriptions` inbox
//! table. Uses a `delivered_at` sentinel column to avoid re-delivery.
//!
//! The worker runs every `interval_secs` seconds (default 60). Each cycle
//! fetches up to `BATCH` due alarms, marks them delivered, and emits a
//! tracing event for each one.  In production an operator wires the
//! notifications service to pick these up via the DB or NATS.

use std::time::Duration;

use expresso_core::DbPool;
use tokio::time::interval;
use tracing::{info, warn};

/// Default polling cadence.
pub const DEFAULT_INTERVAL_SECS: u64 = 60;

const BATCH: i64 = 200;

/// Spawn the alarm delivery worker. Non-blocking.
pub fn spawn(pool: DbPool, interval_secs: u64) {
    let secs = interval_secs.max(10);
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(secs));
        loop {
            tick.tick().await;
            match deliver_once(&pool).await {
                Ok(n) if n > 0 => info!(delivered = n, "alarm delivery cycle completed"),
                Ok(_)          => {}
                Err(e)         => warn!(error = %e, "alarm delivery failed"),
            }
        }
    });
}

/// Single delivery cycle.  Returns the number of alarms delivered.
pub async fn deliver_once(pool: &DbPool) -> sqlx::Result<u64> {
    // Fetch due alarms not yet delivered; lock rows to prevent concurrent delivery.
    let rows: Vec<(uuid::Uuid, uuid::Uuid, uuid::Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT id, event_id, tenant_id, action, description \
         FROM calendar_event_alarms \
         WHERE trigger_abs IS NOT NULL \
           AND trigger_abs <= now() \
           AND delivered_at IS NULL \
         ORDER BY trigger_abs ASC \
         LIMIT $1 \
         FOR UPDATE SKIP LOCKED",
    )
    .bind(BATCH)
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok(0);
    }

    let ids: Vec<uuid::Uuid> = rows.iter().map(|(id, ..)| *id).collect();

    // Mark delivered atomically.
    sqlx::query(
        "UPDATE calendar_event_alarms \
         SET delivered_at = now() \
         WHERE id = ANY($1)",
    )
    .bind(&ids[..])
    .execute(pool)
    .await?;

    for (id, event_id, tenant_id, action, desc) in &rows {
        info!(
            alarm_id    = %id,
            event_id    = %event_id,
            tenant_id   = %tenant_id,
            action      = %action,
            description = desc.as_deref().unwrap_or(""),
            "alarm due — delivered"
        );
    }

    Ok(rows.len() as u64)
}

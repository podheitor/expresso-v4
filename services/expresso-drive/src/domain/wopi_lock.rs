//! WOPI lock state — MS-WOPI Lock/Unlock/RefreshLock/UnlockAndRelock.
//!
//! WOPI locks are advisory and last 30 minutes by default; clients call
//! RefreshLock to keep them alive. Expired locks are treated as absent
//! by every read (no GC needed for correctness; a future periodic purge
//! can clean up rows for ergonomics).

use expresso_core::DbPool;
use sqlx::FromRow;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::error::Result;

/// WOPI spec: lock lifetime is 30 minutes from acquire/refresh.
pub const LOCK_TTL: Duration = Duration::minutes(30);

#[derive(Debug, Clone, FromRow)]
pub struct WopiLock {
    pub file_id:     Uuid,
    pub tenant_id:   Uuid,
    pub lock_token:  String,
    pub locked_by:   Uuid,
    pub acquired_at: OffsetDateTime,
    pub expires_at:  OffsetDateTime,
}

impl WopiLock {
    pub fn is_expired(&self) -> bool {
        self.expires_at <= OffsetDateTime::now_utc()
    }
}

pub struct WopiLockRepo<'a> { pool: &'a DbPool }

const COLS: &str = "file_id, tenant_id, lock_token, locked_by, acquired_at, expires_at";

impl<'a> WopiLockRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    /// Returns the lock row if present AND not expired. An expired lock is
    /// treated as absent (caller can overwrite it via `acquire`).
    pub async fn get_active(&self, tenant_id: Uuid, file_id: Uuid) -> Result<Option<WopiLock>> {
        let sql = format!(
            "SELECT {COLS} FROM drive_wopi_locks \
             WHERE tenant_id = $1 AND file_id = $2 AND expires_at > now()"
        );
        let row: Option<WopiLock> = sqlx::query_as(&sql)
            .bind(tenant_id).bind(file_id)
            .fetch_optional(self.pool).await?;
        Ok(row)
    }

    /// Insert or atomically refresh a lock when the caller-supplied token
    /// matches (or when the existing lock is expired). Returns the resulting
    /// row. When a different active lock already exists, returns Ok(None) —
    /// caller surfaces 409 with the existing X-WOPI-Lock.
    pub async fn acquire_or_refresh(
        &self,
        tenant_id: Uuid,
        file_id:   Uuid,
        token:     &str,
        user_id:   Uuid,
    ) -> Result<AcquireOutcome> {
        // Single-statement upsert: insert when absent or when active lock
        // belongs to the same token (refresh); reject when a different
        // active token already holds the file. ON CONFLICT branch covers
        // both replace-expired and refresh-same-token cases.
        let sql = format!(
            "INSERT INTO drive_wopi_locks (file_id, tenant_id, lock_token, locked_by, expires_at) \
             VALUES ($1, $2, $3, $4, now() + INTERVAL '30 minutes') \
             ON CONFLICT (file_id) DO UPDATE \
                 SET lock_token  = EXCLUDED.lock_token, \
                     locked_by   = EXCLUDED.locked_by, \
                     acquired_at = CASE \
                         WHEN drive_wopi_locks.lock_token = EXCLUDED.lock_token \
                              AND drive_wopi_locks.expires_at > now() \
                         THEN drive_wopi_locks.acquired_at \
                         ELSE now() END, \
                     expires_at  = now() + INTERVAL '30 minutes' \
                 WHERE drive_wopi_locks.expires_at <= now() \
                    OR drive_wopi_locks.lock_token = EXCLUDED.lock_token \
             RETURNING {COLS}"
        );
        let row: Option<WopiLock> = sqlx::query_as(&sql)
            .bind(file_id).bind(tenant_id).bind(token).bind(user_id)
            .fetch_optional(self.pool).await?;
        match row {
            Some(lock) => Ok(AcquireOutcome::Held(lock)),
            None => {
                // Different active lock blocked the upsert.
                let existing = self.get_active(tenant_id, file_id).await?;
                Ok(AcquireOutcome::Conflict(existing))
            }
        }
    }

    /// Atomic UnlockAndRelock: only succeeds when the active lock matches
    /// `old_token`. Same conflict semantics as `acquire_or_refresh`.
    pub async fn unlock_and_relock(
        &self,
        tenant_id: Uuid,
        file_id:   Uuid,
        old_token: &str,
        new_token: &str,
        user_id:   Uuid,
    ) -> Result<AcquireOutcome> {
        let sql = format!(
            "UPDATE drive_wopi_locks \
                SET lock_token  = $4, \
                    locked_by   = $5, \
                    acquired_at = now(), \
                    expires_at  = now() + INTERVAL '30 minutes' \
              WHERE tenant_id = $1 AND file_id = $2 \
                AND lock_token = $3 AND expires_at > now() \
              RETURNING {COLS}"
        );
        let row: Option<WopiLock> = sqlx::query_as(&sql)
            .bind(tenant_id).bind(file_id).bind(old_token).bind(new_token).bind(user_id)
            .fetch_optional(self.pool).await?;
        match row {
            Some(lock) => Ok(AcquireOutcome::Held(lock)),
            None => Ok(AcquireOutcome::Conflict(self.get_active(tenant_id, file_id).await?)),
        }
    }

    /// Release a lock. Returns Ok(true) when removed, Ok(false) when the
    /// supplied token didn't match the active lock.
    pub async fn release(
        &self,
        tenant_id: Uuid,
        file_id:   Uuid,
        token:     &str,
    ) -> Result<bool> {
        let r = sqlx::query(
            "DELETE FROM drive_wopi_locks \
              WHERE tenant_id = $1 AND file_id = $2 \
                AND lock_token = $3 AND expires_at > now()"
        )
        .bind(tenant_id).bind(file_id).bind(token)
        .execute(self.pool).await?;
        Ok(r.rows_affected() > 0)
    }
}

/// Result of an acquire/refresh attempt.
#[derive(Debug)]
pub enum AcquireOutcome {
    /// Caller now holds the lock with the supplied token.
    Held(WopiLock),
    /// A different active lock blocked the operation. Inner Option is the
    /// existing row when readable (rare race: another tx could have just
    /// expired it, in which case the caller may retry).
    Conflict(Option<WopiLock>),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(token: &str, expires_in: Duration) -> WopiLock {
        WopiLock {
            file_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            lock_token: token.into(),
            locked_by: Uuid::nil(),
            acquired_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc() + expires_in,
        }
    }

    #[test]
    fn fresh_lock_not_expired() {
        assert!(!lock("t", Duration::minutes(15)).is_expired());
    }

    #[test]
    fn old_lock_is_expired() {
        assert!(lock("t", Duration::minutes(-1)).is_expired());
    }

    #[test]
    fn ttl_is_thirty_minutes() {
        assert_eq!(LOCK_TTL, Duration::minutes(30));
    }
}

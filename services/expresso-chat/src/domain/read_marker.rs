//! Read markers — per-user, per-channel "last read" tracking for unread state.
//!
//! Unread is a *status* (boolean), not a count: chat messages live in Matrix,
//! so an exact count would need an HS round-trip per channel. Instead we bump
//! `chat_channels.updated_at` on each send (`bump_activity`) and treat a channel
//! as unread for a user when its `updated_at` is newer than that user's
//! `last_read_at` (or no marker exists). All writes go through `begin_tenant_tx`
//! so the `chat_read_markers` RLS policy scopes by `app.tenant_id`.

use uuid::Uuid;

use expresso_core::{begin_tenant_tx, DbPool};

use crate::error::Result;

pub struct ReadMarkerRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> ReadMarkerRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Mark a channel read for a user up to "now". Idempotent upsert; advancing
    /// only forward (a stale client can't move the marker backwards).
    pub async fn mark_read(&self, tenant: Uuid, channel: Uuid, user: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        sqlx::query(
            r#"INSERT INTO chat_read_markers (channel_id, tenant_id, user_id, last_read_at)
               VALUES ($1, $2, $3, now())
               ON CONFLICT (channel_id, user_id)
               DO UPDATE SET last_read_at = GREATEST(chat_read_markers.last_read_at, now())"#,
        )
        .bind(channel)
        .bind(tenant)
        .bind(user)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Bump a channel's activity timestamp so it shows unread for everyone who
    /// hasn't read since. Called after a message is sent. Best-effort: the
    /// `updated_at` touch trigger sets the new value.
    pub async fn bump_activity(&self, tenant: Uuid, channel: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        sqlx::query(r#"UPDATE chat_channels SET updated_at = now() WHERE tenant_id=$1 AND id=$2"#)
            .bind(tenant)
            .bind(channel)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Channel ids that are unread for `user`: non-archived channels the user is
    /// a member of, whose latest activity is newer than the user's read marker
    /// (or which the user has never read). Excludes channels last touched by the
    /// user's own send within the same second is *not* attempted — `mark_read`
    /// on send-success keeps a sender's own channel read.
    pub async fn unread_channels(&self, tenant: Uuid, user: Uuid) -> Result<Vec<Uuid>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            r#"SELECT c.id
               FROM chat_channels c
               JOIN chat_channel_members m
                 ON m.channel_id = c.id AND m.tenant_id = c.tenant_id
               LEFT JOIN chat_read_markers r
                 ON r.channel_id = c.id AND r.user_id = m.user_id
               WHERE c.tenant_id = $1
                 AND m.user_id = $2
                 AND c.is_archived = FALSE
                 AND (r.last_read_at IS NULL OR c.updated_at > r.last_read_at)"#,
        )
        .bind(tenant)
        .bind(user)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }
}

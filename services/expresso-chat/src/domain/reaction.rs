//! Reactions — emoji reactions on chat messages.
//!
//! Stored locally (messages live in Matrix) so the UI can aggregate per-message
//! reactions in one query. All writes go through `begin_tenant_tx` so the
//! `chat_reactions` RLS policy scopes by `app.tenant_id`.

use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use expresso_core::{begin_tenant_tx, DbPool};

use crate::error::Result;

/// One aggregated reaction bucket for a message: an emoji, how many users used
/// it, and the list of those users (so a client can render "you + 2 others").
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ReactionCount {
    pub event_id: String,
    pub emoji: String,
    pub count: i64,
    pub users: Vec<Uuid>,
}

pub struct ReactionRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> ReactionRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Add a reaction. Idempotent: re-reacting with the same emoji is a no-op.
    /// Returns `true` when a new row was inserted (so the caller only broadcasts
    /// on a real change).
    pub async fn add(
        &self,
        tenant: Uuid,
        channel: Uuid,
        event_id: &str,
        user: Uuid,
        emoji: &str,
    ) -> Result<bool> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let res = sqlx::query(
            r#"INSERT INTO chat_reactions (channel_id, tenant_id, event_id, user_id, emoji)
               VALUES ($1, $2, $3, $4, $5)
               ON CONFLICT (channel_id, event_id, user_id, emoji) DO NOTHING"#,
        )
        .bind(channel)
        .bind(tenant)
        .bind(event_id)
        .bind(user)
        .bind(emoji)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    /// Remove a reaction. Returns `true` when a row was actually deleted.
    pub async fn remove(
        &self,
        tenant: Uuid,
        channel: Uuid,
        event_id: &str,
        user: Uuid,
        emoji: &str,
    ) -> Result<bool> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let res = sqlx::query(
            r#"DELETE FROM chat_reactions
               WHERE channel_id=$1 AND event_id=$2 AND user_id=$3 AND emoji=$4"#,
        )
        .bind(channel)
        .bind(event_id)
        .bind(user)
        .bind(emoji)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    /// Aggregate reactions for a set of message ids in one channel: one row per
    /// (event_id, emoji) with the count and the reacting users. Empty input
    /// returns an empty vec without touching the DB.
    pub async fn list_for_events(
        &self,
        tenant: Uuid,
        channel: Uuid,
        event_ids: &[String],
    ) -> Result<Vec<ReactionCount>> {
        if event_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows: Vec<ReactionCount> = sqlx::query_as(
            r#"SELECT event_id,
                      emoji,
                      COUNT(*)            AS count,
                      array_agg(user_id)  AS users
               FROM chat_reactions
               WHERE channel_id = $1 AND event_id = ANY($2)
               GROUP BY event_id, emoji
               ORDER BY event_id, emoji"#,
        )
        .bind(channel)
        .bind(event_ids)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }
}

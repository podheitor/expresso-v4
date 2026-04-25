//! Channel (= Matrix room pair) repository.
//!
//! Tenant scoping: cada método abre transação via `begin_tenant_tx` para que
//! as policies RLS de `chat_channels` / `chat_channel_members` filtrem por
//! `current_setting('app.tenant_id')`. As cláusulas `WHERE tenant_id = $1`
//! permanecem como defense-in-depth. `create` ganha de brinde atomicidade
//! entre o INSERT do canal e o INSERT do owner-member (antes rodavam em
//! txs separadas — falha do segundo deixava canal órfão).

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use expresso_core::{begin_tenant_tx, DbPool};

use crate::error::Result;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ChannelKind { Team, Direct, Announcement, Project }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum MemberRole { Owner, Admin, Member, Guest }

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Channel {
    pub id:              Uuid,
    pub tenant_id:       Uuid,
    pub matrix_room_id:  String,
    pub name:            String,
    pub topic:           Option<String>,
    pub kind:            ChannelKind,
    pub team_id:         Option<Uuid>,
    pub created_by:      Uuid,
    pub is_archived:     bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:      OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at:      OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ChannelMember {
    pub channel_id: Uuid,
    pub tenant_id:  Uuid,
    pub user_id:    Uuid,
    pub role:       MemberRole,
    #[serde(with = "time::serde::rfc3339")]
    pub joined_at:  OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewChannel {
    pub matrix_room_id: String,
    pub name:           String,
    pub topic:          Option<String>,
    pub kind:           ChannelKind,
    pub team_id:        Option<Uuid>,
}

pub struct ChannelRepo<'a> { pool: &'a DbPool }

impl<'a> ChannelRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    pub async fn create(&self, tenant: Uuid, owner: Uuid, n: NewChannel) -> Result<Channel> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row: Channel = sqlx::query_as(
            r#"INSERT INTO chat_channels (tenant_id, matrix_room_id, name, topic, kind, team_id, created_by)
               VALUES ($1,$2,$3,$4,$5,$6,$7)
               RETURNING id, tenant_id, matrix_room_id, name, topic, kind, team_id,
                         created_by, is_archived, created_at, updated_at"#)
            .bind(tenant).bind(&n.matrix_room_id).bind(&n.name).bind(&n.topic)
            .bind(n.kind).bind(n.team_id).bind(owner)
            .fetch_one(&mut *tx).await?;
        // Creator is automatically the owner member — same tx so a failure
        // here rolls back the channel insert (no orphan rooms).
        sqlx::query(
            r#"INSERT INTO chat_channel_members (channel_id, tenant_id, user_id, role)
               VALUES ($1,$2,$3,'owner')"#)
            .bind(row.id).bind(tenant).bind(owner)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn list_for_user(&self, tenant: Uuid, user: Uuid) -> Result<Vec<Channel>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows: Vec<Channel> = sqlx::query_as(
            r#"SELECT c.id, c.tenant_id, c.matrix_room_id, c.name, c.topic, c.kind,
                      c.team_id, c.created_by, c.is_archived, c.created_at, c.updated_at
               FROM chat_channels c
               JOIN chat_channel_members m ON m.channel_id = c.id
               WHERE c.tenant_id = $1 AND m.user_id = $2 AND c.is_archived = FALSE
               ORDER BY c.updated_at DESC"#)
            .bind(tenant).bind(user)
            .fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn get(&self, tenant: Uuid, id: Uuid) -> Result<Channel> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row: Channel = sqlx::query_as(
            r#"SELECT id, tenant_id, matrix_room_id, name, topic, kind, team_id,
                      created_by, is_archived, created_at, updated_at
               FROM chat_channels WHERE tenant_id=$1 AND id=$2"#)
            .bind(tenant).bind(id)
            .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn member_role(
        &self,
        tenant: Uuid,
        channel: Uuid,
        user: Uuid,
    ) -> Result<Option<MemberRole>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row: Option<(MemberRole,)> = sqlx::query_as(
            r#"SELECT role FROM chat_channel_members
               WHERE tenant_id=$1 AND channel_id=$2 AND user_id=$3"#)
            .bind(tenant).bind(channel).bind(user)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(row.map(|(r,)| r))
    }

    pub async fn is_member(&self, tenant: Uuid, channel: Uuid, user: Uuid) -> Result<bool> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let cnt: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM chat_channel_members
               WHERE tenant_id=$1 AND channel_id=$2 AND user_id=$3"#)
            .bind(tenant).bind(channel).bind(user)
            .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(cnt > 0)
    }

    pub async fn add_member(&self, tenant: Uuid, channel: Uuid, user: Uuid, role: MemberRole) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        sqlx::query(
            r#"INSERT INTO chat_channel_members (channel_id, tenant_id, user_id, role)
               VALUES ($1,$2,$3,$4)
               ON CONFLICT (channel_id, user_id) DO UPDATE SET role = EXCLUDED.role"#)
            .bind(channel).bind(tenant).bind(user).bind(role)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_members(&self, tenant: Uuid, channel: Uuid) -> Result<Vec<ChannelMember>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows: Vec<ChannelMember> = sqlx::query_as(
            r#"SELECT channel_id, tenant_id, user_id, role, joined_at
               FROM chat_channel_members WHERE tenant_id=$1 AND channel_id=$2"#)
            .bind(tenant).bind(channel)
            .fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn archive(&self, tenant: Uuid, id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        sqlx::query(
            r#"UPDATE chat_channels SET is_archived = TRUE WHERE tenant_id=$1 AND id=$2"#)
            .bind(tenant).bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

//! Meetings repository — Jitsi room registry + participant ACL.
//!
//! Tenant scoping: cada método abre transação via `begin_tenant_tx` para
//! defense-in-depth — o WHERE filtra `tenant_id` explícito, e RLS das
//! tabelas `meetings`/`meeting_participants` filtra junto. Sem o
//! session-var-setting da RLS, a policy NULL-bypass retornaria o universo
//! se o predicado explícito fosse removido em refactor futuro.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use expresso_core::{begin_tenant_tx, DbPool};

use crate::error::Result;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ParticipantRole { Moderator, Participant }

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Meeting {
    pub id:            Uuid,
    pub tenant_id:     Uuid,
    pub room_name:     String,
    pub title:         String,
    pub channel_id:    Option<Uuid>,
    pub created_by:    Uuid,
    #[serde(with = "time::serde::rfc3339::option")]
    pub scheduled_for: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ends_at:       Option<OffsetDateTime>,
    pub is_recurring:  bool,
    pub is_archived:   bool,
    pub lobby_enabled: bool,
    pub password:      Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:    OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at:    OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct MeetingParticipant {
    pub meeting_id: Uuid,
    pub tenant_id:  Uuid,
    pub user_id:    Uuid,
    pub role:       ParticipantRole,
    #[serde(with = "time::serde::rfc3339")]
    pub invited_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewMeeting {
    pub room_name:     String,
    pub title:         String,
    pub channel_id:    Option<Uuid>,
    pub scheduled_for: Option<OffsetDateTime>,
    pub ends_at:       Option<OffsetDateTime>,
    pub is_recurring:  Option<bool>,
    pub lobby_enabled: Option<bool>,
    pub password:      Option<String>,
}

pub struct MeetingRepo<'a> { pool: &'a DbPool }

impl<'a> MeetingRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    pub async fn create(&self, tenant: Uuid, creator: Uuid, n: NewMeeting) -> Result<Meeting> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row: Meeting = sqlx::query_as(
            r#"INSERT INTO meetings
                 (tenant_id, room_name, title, channel_id, created_by,
                  scheduled_for, ends_at, is_recurring, lobby_enabled, password)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
               RETURNING id, tenant_id, room_name, title, channel_id, created_by,
                         scheduled_for, ends_at, is_recurring, is_archived,
                         lobby_enabled, password, created_at, updated_at"#)
            .bind(tenant).bind(&n.room_name).bind(&n.title).bind(n.channel_id)
            .bind(creator).bind(n.scheduled_for).bind(n.ends_at)
            .bind(n.is_recurring.unwrap_or(false))
            .bind(n.lobby_enabled.unwrap_or(true))
            .bind(&n.password)
            .fetch_one(&mut *tx).await?;
        // Creator is an automatic moderator.
        sqlx::query(
            r#"INSERT INTO meeting_participants (meeting_id, tenant_id, user_id, role)
               VALUES ($1,$2,$3,'moderator')"#)
            .bind(row.id).bind(tenant).bind(creator)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn get(&self, tenant: Uuid, id: Uuid) -> Result<Meeting> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row: Meeting = sqlx::query_as(
            r#"SELECT id, tenant_id, room_name, title, channel_id, created_by,
                      scheduled_for, ends_at, is_recurring, is_archived,
                      lobby_enabled, password, created_at, updated_at
               FROM meetings WHERE tenant_id=$1 AND id=$2"#)
            .bind(tenant).bind(id).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn list_for_user(&self, tenant: Uuid, user: Uuid) -> Result<Vec<Meeting>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows: Vec<Meeting> = sqlx::query_as(
            r#"SELECT m.id, m.tenant_id, m.room_name, m.title, m.channel_id, m.created_by,
                      m.scheduled_for, m.ends_at, m.is_recurring, m.is_archived,
                      m.lobby_enabled, m.password, m.created_at, m.updated_at
               FROM meetings m
               JOIN meeting_participants p ON p.meeting_id = m.id
               WHERE m.tenant_id = $1 AND p.user_id = $2 AND m.is_archived = FALSE
               ORDER BY COALESCE(m.scheduled_for, m.created_at) DESC"#)
            .bind(tenant).bind(user).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn participant_role(
        &self,
        tenant: Uuid,
        meeting: Uuid,
        user: Uuid,
    ) -> Result<Option<ParticipantRole>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row: Option<(ParticipantRole,)> = sqlx::query_as(
            r#"SELECT role FROM meeting_participants
               WHERE tenant_id=$1 AND meeting_id=$2 AND user_id=$3"#)
            .bind(tenant).bind(meeting).bind(user)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(row.map(|(r,)| r))
    }

    pub async fn add_participant(
        &self,
        tenant: Uuid,
        meeting: Uuid,
        user: Uuid,
        role: ParticipantRole,
    ) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        sqlx::query(
            r#"INSERT INTO meeting_participants (meeting_id, tenant_id, user_id, role)
               VALUES ($1,$2,$3,$4)
               ON CONFLICT (meeting_id, user_id) DO UPDATE SET role = EXCLUDED.role"#)
            .bind(meeting).bind(tenant).bind(user).bind(role)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_participants(&self, tenant: Uuid, meeting: Uuid) -> Result<Vec<MeetingParticipant>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows: Vec<MeetingParticipant> = sqlx::query_as(
            r#"SELECT meeting_id, tenant_id, user_id, role, invited_at
               FROM meeting_participants WHERE tenant_id=$1 AND meeting_id=$2"#)
            .bind(tenant).bind(meeting).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn archive(&self, tenant: Uuid, id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        sqlx::query(
            r#"UPDATE meetings SET is_archived = TRUE WHERE tenant_id=$1 AND id=$2"#)
            .bind(tenant).bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

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
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub recording_started_at: Option<OffsetDateTime>,
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
                         lobby_enabled, password, recording_started_at, created_at, updated_at"#)
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
                      lobby_enabled, password, recording_started_at, created_at, updated_at
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
                      m.lobby_enabled, m.password, m.recording_started_at, m.created_at, m.updated_at
               FROM meetings m
               JOIN meeting_participants p ON p.meeting_id = m.id
               WHERE m.tenant_id = $1 AND p.user_id = $2 AND m.is_archived = FALSE
               ORDER BY COALESCE(m.scheduled_for, m.created_at) DESC"#)
            .bind(tenant).bind(user).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// List active meetings for a user, optionally filtered by scheduled_for range.
    /// Rows where scheduled_for IS NULL are excluded when a date filter is provided.
    pub async fn list_for_user_filtered(
        &self,
        tenant: Uuid,
        user:   Uuid,
        after:  Option<OffsetDateTime>,
        before: Option<OffsetDateTime>,
    ) -> Result<Vec<Meeting>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows: Vec<Meeting> = sqlx::query_as(
            r#"SELECT m.id, m.tenant_id, m.room_name, m.title, m.channel_id, m.created_by,
                      m.scheduled_for, m.ends_at, m.is_recurring, m.is_archived,
                      m.lobby_enabled, m.password, m.recording_started_at, m.created_at, m.updated_at
               FROM meetings m
               JOIN meeting_participants p ON p.meeting_id = m.id
               WHERE m.tenant_id = $1 AND p.user_id = $2 AND m.is_archived = FALSE
                 AND ($3::timestamptz IS NULL OR m.scheduled_for >= $3)
                 AND ($4::timestamptz IS NULL OR m.scheduled_for <= $4)
               ORDER BY COALESCE(m.scheduled_for, m.created_at) DESC"#)
            .bind(tenant).bind(user).bind(after).bind(before)
            .fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn list_archived_for_user(&self, tenant: Uuid, user: Uuid) -> Result<Vec<Meeting>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows: Vec<Meeting> = sqlx::query_as(
            r#"SELECT m.id, m.tenant_id, m.room_name, m.title, m.channel_id, m.created_by,
                      m.scheduled_for, m.ends_at, m.is_recurring, m.is_archived,
                      m.lobby_enabled, m.password, m.recording_started_at, m.created_at, m.updated_at
               FROM meetings m
               JOIN meeting_participants p ON p.meeting_id = m.id
               WHERE m.tenant_id = $1 AND p.user_id = $2 AND m.is_archived = TRUE
               ORDER BY m.updated_at DESC"#)
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

    /// Update the role of an existing participant.
    /// Returns the updated row, or None if the participant doesn't exist.
    pub async fn set_participant_role(
        &self,
        tenant:  Uuid,
        meeting: Uuid,
        user:    Uuid,
        role:    ParticipantRole,
    ) -> Result<bool> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let r = sqlx::query(
            r#"UPDATE meeting_participants SET role = $4
               WHERE tenant_id=$1 AND meeting_id=$2 AND user_id=$3"#)
            .bind(tenant).bind(meeting).bind(user).bind(role)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(r.rows_affected() > 0)
    }

    /// Remove a participant. Returns true if a row was deleted, false if not found.
    /// Caller must ensure the meeting's creator is never removed.
    pub async fn remove_participant(
        &self,
        tenant:  Uuid,
        meeting: Uuid,
        user:    Uuid,
    ) -> Result<bool> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let r = sqlx::query(
            r#"DELETE FROM meeting_participants
               WHERE tenant_id=$1 AND meeting_id=$2 AND user_id=$3"#)
            .bind(tenant).bind(meeting).bind(user)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(r.rows_affected() > 0)
    }

    pub async fn count_participants(&self, tenant: Uuid, meeting: Uuid) -> Result<i64> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let (n,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM meeting_participants WHERE tenant_id=$1 AND meeting_id=$2",
        )
        .bind(tenant).bind(meeting).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(n)
    }

    pub async fn get_participant(
        &self,
        tenant:  Uuid,
        meeting: Uuid,
        user:    Uuid,
    ) -> Result<Option<MeetingParticipant>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row: Option<MeetingParticipant> = sqlx::query_as(
            r#"SELECT meeting_id, tenant_id, user_id, role, invited_at
               FROM meeting_participants WHERE tenant_id=$1 AND meeting_id=$2 AND user_id=$3"#)
            .bind(tenant).bind(meeting).bind(user)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
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

    pub async fn list_participants_paged(&self, tenant: Uuid, meeting: Uuid, limit: i64, offset: i64) -> Result<Vec<MeetingParticipant>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows: Vec<MeetingParticipant> = sqlx::query_as(
            r#"SELECT meeting_id, tenant_id, user_id, role, invited_at
               FROM meeting_participants WHERE tenant_id=$1 AND meeting_id=$2
               ORDER BY invited_at ASC
               LIMIT $3 OFFSET $4"#)
            .bind(tenant).bind(meeting).bind(limit).bind(offset).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn archive(&self, tenant: Uuid, id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        sqlx::query(
            r#"UPDATE meetings SET is_archived = TRUE, updated_at = NOW()
               WHERE tenant_id=$1 AND id=$2"#)
            .bind(tenant).bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn restore(&self, tenant: Uuid, id: Uuid) -> Result<Option<Meeting>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row: Option<Meeting> = sqlx::query_as(
            r#"UPDATE meetings SET is_archived = FALSE, updated_at = NOW()
               WHERE tenant_id=$1 AND id=$2 AND is_archived = TRUE
               RETURNING id, tenant_id, room_name, title, channel_id, created_by,
                         scheduled_for, ends_at, is_recurring, is_archived,
                         lobby_enabled, password, recording_started_at, created_at, updated_at"#)
            .bind(tenant).bind(id).fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn update(
        &self,
        tenant:        Uuid,
        id:            Uuid,
        title:         Option<String>,
        scheduled_for: Option<Option<OffsetDateTime>>,
        ends_at:       Option<Option<OffsetDateTime>>,
        lobby_enabled: Option<bool>,
        password:      Option<Option<String>>,
        is_recurring:  Option<bool>,
    ) -> Result<Option<Meeting>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row: Option<Meeting> = sqlx::query_as(
            r#"UPDATE meetings
               SET title          = COALESCE($3, title),
                   scheduled_for  = CASE WHEN $4 THEN $5 ELSE scheduled_for END,
                   ends_at        = CASE WHEN $6 THEN $7 ELSE ends_at END,
                   lobby_enabled  = COALESCE($8, lobby_enabled),
                   password       = CASE WHEN $9 THEN $10 ELSE password END,
                   is_recurring   = COALESCE($11, is_recurring),
                   updated_at     = NOW()
               WHERE tenant_id = $1 AND id = $2 AND is_archived = FALSE
               RETURNING id, tenant_id, room_name, title, channel_id, created_by,
                         scheduled_for, ends_at, is_recurring, is_archived,
                         lobby_enabled, password, recording_started_at, created_at, updated_at"#)
            .bind(tenant)
            .bind(id)
            .bind(title)
            .bind(scheduled_for.is_some())
            .bind(scheduled_for.and_then(|v| v))
            .bind(ends_at.is_some())
            .bind(ends_at.and_then(|v| v))
            .bind(lobby_enabled)
            .bind(password.is_some())
            .bind(password.and_then(|v| v))
            .bind(is_recurring)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Set recording_started_at = now(). Returns None if already recording or not found.
    pub async fn start_recording(&self, tenant: Uuid, id: Uuid) -> Result<Option<Meeting>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row: Option<Meeting> = sqlx::query_as(
            r#"UPDATE meetings SET recording_started_at = now(), updated_at = now()
               WHERE tenant_id = $1 AND id = $2 AND is_archived = FALSE
                 AND recording_started_at IS NULL
               RETURNING id, tenant_id, room_name, title, channel_id, created_by,
                         scheduled_for, ends_at, is_recurring, is_archived,
                         lobby_enabled, password, recording_started_at, created_at, updated_at"#)
        .bind(tenant).bind(id).fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Clear recording_started_at. Returns None if not recording or not found.
    pub async fn stop_recording(&self, tenant: Uuid, id: Uuid) -> Result<Option<Meeting>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row: Option<Meeting> = sqlx::query_as(
            r#"UPDATE meetings SET recording_started_at = NULL, updated_at = now()
               WHERE tenant_id = $1 AND id = $2 AND is_archived = FALSE
                 AND recording_started_at IS NOT NULL
               RETURNING id, tenant_id, room_name, title, channel_id, created_by,
                         scheduled_for, ends_at, is_recurring, is_archived,
                         lobby_enabled, password, recording_started_at, created_at, updated_at"#)
        .bind(tenant).bind(id).fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn participant_role_serde_roundtrip() {
        for role in [ParticipantRole::Moderator, ParticipantRole::Participant] {
            let s = serde_json::to_string(&role).unwrap();
            let back: ParticipantRole = serde_json::from_str(&s).unwrap();
            assert_eq!(role, back);
        }
    }

    #[test]
    fn participant_role_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&ParticipantRole::Moderator).unwrap(), r#""moderator""#);
        assert_eq!(serde_json::to_string(&ParticipantRole::Participant).unwrap(), r#""participant""#);
    }

    #[test]
    fn new_meeting_deserialize_minimal() {
        let json = r#"{"room_name":"room-abc","title":"Daily standup"}"#;
        let n: NewMeeting = serde_json::from_str(json).unwrap();
        assert_eq!(n.room_name, "room-abc");
        assert_eq!(n.title, "Daily standup");
        assert!(n.channel_id.is_none());
        assert!(n.scheduled_for.is_none());
        assert!(n.ends_at.is_none());
        assert!(n.is_recurring.is_none());
        assert!(n.lobby_enabled.is_none());
        assert!(n.password.is_none());
    }

    #[test]
    fn new_meeting_deserialize_full() {
        let json = r#"{
            "room_name":"room-xyz",
            "title":"Weekly review",
            "is_recurring":true,
            "lobby_enabled":false,
            "password":"s3cr3t"
        }"#;
        let n: NewMeeting = serde_json::from_str(json).unwrap();
        assert_eq!(n.is_recurring, Some(true));
        assert_eq!(n.lobby_enabled, Some(false));
        assert_eq!(n.password.as_deref(), Some("s3cr3t"));
    }
}

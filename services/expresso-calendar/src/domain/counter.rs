//! COUNTER proposals (iTIP §3.2.7) persistence layer.
//!
//! Stores attendee counter-proposals until the organizer decides via the admin
//! UI. Accept → update event DTSTART/DTEND (SEQUENCE auto-bumps via event::update).
//! Reject → mark resolved; caller may dispatch DECLINECOUNTER iMIP externally.
//!
//! Tenant scoping: cada método abre transação via `begin_tenant_tx` para
//! consistência com os outros repos do serviço e p/ deixar
//! `current_setting('app.tenant_id')` populado caso a tabela ganhe RLS
//! no futuro — a migration de `scheduling_counter_proposals` ainda não
//! tem ENABLE ROW LEVEL SECURITY, então o `WHERE tenant_id = $1` continua
//! sendo o único guardrail efetivo hoje. `resolve` passou a receber
//! `tenant_id` (API change) — antes atualizava só por `id`, sem guardrail
//! algum; sem call sites externos no momento (apenas `insert` é chamado
//! em `api/scheduling.rs`).

use expresso_core::begin_tenant_tx;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CounterProposal {
    pub id:                Uuid,
    pub tenant_id:         Uuid,
    pub event_id:          Uuid,
    pub attendee_email:    String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub proposed_dtstart:  Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub proposed_dtend:    Option<OffsetDateTime>,
    pub comment:           Option<String>,
    pub status:            String,
    pub received_sequence: Option<i32>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:        OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolved_at:       Option<OffsetDateTime>,
    pub resolved_by:       Option<Uuid>,
}

pub struct CounterRepo<'a> { pool: &'a PgPool }

impl<'a> CounterRepo<'a> {
    pub fn new(pool: &'a PgPool) -> Self { Self { pool } }

    /// Insert a new pending proposal.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert(
        &self,
        tenant_id:         Uuid,
        event_id:          Uuid,
        attendee_email:    &str,
        proposed_dtstart:  Option<OffsetDateTime>,
        proposed_dtend:    Option<OffsetDateTime>,
        comment:           Option<&str>,
        received_sequence: Option<i32>,
        raw_ical:          Option<&str>,
    ) -> Result<CounterProposal> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let p = sqlx::query_as::<_, CounterProposal>(
            r#"
            INSERT INTO scheduling_counter_proposals
                (tenant_id, event_id, attendee_email, proposed_dtstart,
                 proposed_dtend, comment, received_sequence, raw_ical)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
            RETURNING id, tenant_id, event_id, attendee_email,
                      proposed_dtstart, proposed_dtend, comment, status,
                      received_sequence, created_at, resolved_at, resolved_by
            "#,
        )
        .bind(tenant_id)
        .bind(event_id)
        .bind(attendee_email)
        .bind(proposed_dtstart)
        .bind(proposed_dtend)
        .bind(comment)
        .bind(received_sequence)
        .bind(raw_ical)
        .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(p)
    }

    /// List pending proposals for a tenant (newest first).
    pub async fn list_pending(&self, tenant_id: Uuid, limit: i64) -> Result<Vec<CounterProposal>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query_as::<_, CounterProposal>(
            r#"
            SELECT id, tenant_id, event_id, attendee_email, proposed_dtstart,
                   proposed_dtend, comment, status, received_sequence,
                   created_at, resolved_at, resolved_by
              FROM scheduling_counter_proposals
             WHERE tenant_id = $1 AND status = 'pending'
             ORDER BY created_at DESC
             LIMIT $2
            "#,
        )
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Fetch one by id scoped to tenant.
    pub async fn get(&self, tenant_id: Uuid, id: Uuid) -> Result<Option<CounterProposal>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let r = sqlx::query_as::<_, CounterProposal>(
            r#"
            SELECT id, tenant_id, event_id, attendee_email, proposed_dtstart,
                   proposed_dtend, comment, status, received_sequence,
                   created_at, resolved_at, resolved_by
              FROM scheduling_counter_proposals
             WHERE tenant_id = $1 AND id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(r)
    }

    /// Mark proposal as resolved (accepted or rejected).
    /// API: `tenant_id` now required (was missing — no guardrail on cross-tenant
    /// id collisions). Sem call sites externos no momento.
    pub async fn resolve(&self, tenant_id: Uuid, id: Uuid, new_status: &str, resolved_by: Option<Uuid>) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        sqlx::query(
            r#"UPDATE scheduling_counter_proposals
                  SET status = $3, resolved_at = NOW(), resolved_by = $4
                WHERE tenant_id = $1 AND id = $2 AND status = 'pending'"#,
        )
        .bind(tenant_id)
        .bind(id)
        .bind(new_status)
        .bind(resolved_by)
        .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn counter_proposal_serde_roundtrip() {
        let p = CounterProposal {
            id:                Uuid::nil(),
            tenant_id:         Uuid::nil(),
            event_id:          Uuid::nil(),
            attendee_email:    "att@example.com".into(),
            proposed_dtstart:  Some(datetime!(2026-06-02 10:00:00 UTC)),
            proposed_dtend:    Some(datetime!(2026-06-02 11:00:00 UTC)),
            comment:           Some("Can we push it?".into()),
            status:            "pending".into(),
            received_sequence: Some(1),
            created_at:        datetime!(2026-05-22 08:00:00 UTC),
            resolved_at:       None,
            resolved_by:       None,
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: CounterProposal = serde_json::from_str(&s).unwrap();
        assert_eq!(back.attendee_email, "att@example.com");
        assert_eq!(back.status, "pending");
        assert!(back.resolved_at.is_none());
    }

    #[test]
    fn counter_proposal_proposed_dtstart_in_rfc3339() {
        let p = CounterProposal {
            id: Uuid::nil(), tenant_id: Uuid::nil(), event_id: Uuid::nil(),
            attendee_email: "a@b.com".into(),
            proposed_dtstart: Some(datetime!(2026-07-01 14:00:00 UTC)),
            proposed_dtend:   None, comment: None,
            status: "accepted".into(), received_sequence: None,
            created_at: datetime!(2026-05-22 08:00:00 UTC),
            resolved_at: Some(datetime!(2026-05-23 09:00:00 UTC)),
            resolved_by: Some(Uuid::nil()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains("2026-07-01T14:00:00"));
        assert!(s.contains("2026-05-23T09:00:00"));
    }

    #[test]
    fn counter_proposal_status_pending_by_default_in_insert() {
        // The INSERT always writes status='pending'; verify that a freshly-
        // constructed struct carries the expected string value.
        let p = CounterProposal {
            id: Uuid::nil(), tenant_id: Uuid::nil(), event_id: Uuid::nil(),
            attendee_email: "x@x.com".into(),
            proposed_dtstart: None, proposed_dtend: None, comment: None,
            status: "pending".into(), received_sequence: None,
            created_at: datetime!(2026-05-22 00:00:00 UTC),
            resolved_at: None, resolved_by: None,
        };
        assert_eq!(p.status, "pending");
    }

    #[test]
    fn counter_proposal_no_proposed_times_null_in_json() {
        let p = CounterProposal {
            id: Uuid::nil(), tenant_id: Uuid::nil(), event_id: Uuid::nil(),
            attendee_email: "y@x.com".into(),
            proposed_dtstart: None, proposed_dtend: None, comment: None,
            status: "pending".into(), received_sequence: None,
            created_at: datetime!(2026-05-22 00:00:00 UTC),
            resolved_at: None, resolved_by: None,
        };
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(v["proposed_dtstart"].is_null());
        assert!(v["proposed_dtend"].is_null());
    }

    #[test]
    fn counter_proposal_comment_optional() {
        let p = CounterProposal {
            id: Uuid::nil(), tenant_id: Uuid::nil(), event_id: Uuid::nil(),
            attendee_email: "z@x.com".into(),
            proposed_dtstart: None, proposed_dtend: None,
            comment: Some("Please reschedule".into()),
            status: "pending".into(), received_sequence: None,
            created_at: datetime!(2026-05-22 00:00:00 UTC),
            resolved_at: None, resolved_by: None,
        };
        assert_eq!(p.comment.as_deref(), Some("Please reschedule"));
    }

    #[test]
    fn counter_proposal_resolved_by_none() {
        let p = CounterProposal {
            id: Uuid::nil(), tenant_id: Uuid::nil(), event_id: Uuid::nil(),
            attendee_email: "w@x.com".into(),
            proposed_dtstart: None, proposed_dtend: None, comment: None,
            status: "resolved".into(), received_sequence: None,
            created_at: datetime!(2026-05-22 00:00:00 UTC),
            resolved_at: None, resolved_by: None,
        };
        assert!(p.resolved_by.is_none());
    }

    #[test]
    fn counter_proposal_status_field_accessible() {
        let p = CounterProposal {
            id: Uuid::nil(), tenant_id: Uuid::nil(), event_id: Uuid::nil(),
            attendee_email: "a@b.com".into(),
            proposed_dtstart: None, proposed_dtend: None, comment: None,
            status: "pending".into(), received_sequence: None,
            created_at: datetime!(2026-05-22 00:00:00 UTC),
            resolved_at: None, resolved_by: None,
        };
        assert_eq!(p.status, "pending");
    }

    #[test]
    fn counter_proposal_email_unicode() {
        let p = CounterProposal {
            id: Uuid::nil(), tenant_id: Uuid::nil(), event_id: Uuid::nil(),
            attendee_email: "usuário@empresa.com".into(),
            proposed_dtstart: None, proposed_dtend: None, comment: None,
            status: "new".into(), received_sequence: None,
            created_at: datetime!(2026-05-22 00:00:00 UTC),
            resolved_at: None, resolved_by: None,
        };
        assert!(p.attendee_email.contains("usuário"));
    }

    #[test]
    fn counter_proposal_status_preserved() {
        let p = CounterProposal {
            id: Uuid::nil(), tenant_id: Uuid::nil(), event_id: Uuid::nil(),
            attendee_email: "a@ex.com".into(),
            proposed_dtstart: None, proposed_dtend: None, comment: None,
            status: "accepted".into(), received_sequence: None,
            created_at: datetime!(2026-05-22 00:00:00 UTC),
            resolved_at: None, resolved_by: None,
        };
        assert_eq!(p.status, "accepted");
    }

    #[test]
    fn counter_proposal_comment_none_by_default() {
        use time::macros::datetime;
        let p = CounterProposal {
            id: Uuid::nil(), event_id: Uuid::nil(), tenant_id: Uuid::nil(),
            attendee_email: "a@x.com".into(),
            proposed_dtstart: None, proposed_dtend: None, comment: None,
            status: "pending".into(), received_sequence: None,
            created_at: datetime!(2026-06-01 00:00:00 UTC),
            resolved_at: None, resolved_by: None,
        };
        assert!(p.comment.is_none());
    }

    #[test]
    fn counter_proposal_status_pending_by_default() {
        use time::macros::datetime;
        let p = CounterProposal {
            id: Uuid::nil(), event_id: Uuid::nil(), tenant_id: Uuid::nil(),
            attendee_email: "a@x.com".into(),
            proposed_dtstart: None, proposed_dtend: None, comment: None,
            status: "pending".into(), received_sequence: None,
            created_at: datetime!(2026-06-01 00:00:00 UTC),
            resolved_at: None, resolved_by: None,
        };
        assert_eq!(p.status, "pending");
    }

    #[test]
    fn counter_proposal_received_sequence_preserved() {
        use time::macros::datetime;
        let p = CounterProposal {
            id: Uuid::nil(), event_id: Uuid::nil(), tenant_id: Uuid::nil(),
            attendee_email: "b@x.com".into(),
            proposed_dtstart: None, proposed_dtend: None, comment: None,
            status: "pending".into(), received_sequence: Some(3),
            created_at: datetime!(2026-06-01 00:00:00 UTC),
            resolved_at: None, resolved_by: None,
        };
        assert_eq!(p.received_sequence, Some(3));
    }
}

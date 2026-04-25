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

//! Calendar event domain model + repository.
//!
//! Tenant scoping: cada método abre transação via `begin_tenant_tx` para
//! que a policy RLS de `calendar_events` filtre por
//! `current_setting('app.tenant_id')` antes mesmo do `WHERE tenant_id = $1`
//! explícito (defense-in-depth). Fecha o último repo do serviço calendar.

use expresso_core::{begin_tenant_tx, DbPool};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{CalendarError, Result};
use crate::domain::ical;

/// Stored event row. Mirrors `calendar_events` columns.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Event {
    pub id:              Uuid,
    pub calendar_id:     Uuid,
    pub tenant_id:       Uuid,
    pub uid:             String,
    pub etag:            String,
    pub ical_raw:        String,
    pub summary:         Option<String>,
    pub description:     Option<String>,
    pub location:        Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub dtstart:         Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub dtend:           Option<OffsetDateTime>,
    pub rrule:           Option<String>,
    pub status:          Option<String>,
    pub sequence:        i32,
    pub organizer_email: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:      OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at:      OffsetDateTime,
}

/// Time-range query parameters (matches CalDAV calendar-query REPORT).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EventQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub from:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub to:    Option<OffsetDateTime>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Clone)]
pub struct EventRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> EventRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Expose pool for sibling repos in composite flows (e.g. CounterRepo).
    pub fn pool(&self) -> &'a DbPool { self.pool }

    /// Insert an event parsed from raw iCalendar text.
    pub async fn create(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        raw: &str,
    ) -> Result<Event> {
        let parsed = ical::parse_vevent(raw)?;
        let etag   = ical::compute_etag(raw);

        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Event>(
            r#"
            INSERT INTO calendar_events
                (tenant_id, calendar_id, uid, etag, ical_raw, summary, description,
                 location, dtstart, dtend, rrule, status, sequence, organizer_email)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            RETURNING id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                      description, location, dtstart, dtend, rrule, status,
                      sequence, organizer_email, created_at, updated_at
            "#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(&parsed.uid)
        .bind(&etag)
        .bind(raw)
        .bind(&parsed.summary)
        .bind(&parsed.description)
        .bind(&parsed.location)
        .bind(parsed.dtstart)
        .bind(parsed.dtend)
        .bind(&parsed.rrule)
        .bind(&parsed.status)
        .bind(parsed.sequence)
        .bind(&parsed.organizer_email)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }


    /// Fetch single event by id within tenant scope.
    pub async fn get(&self, tenant_id: Uuid, id: Uuid) -> Result<Event> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Event>(
            r#"
            SELECT id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                   description, location, dtstart, dtend, rrule, status,
                   sequence, organizer_email, created_at, updated_at
              FROM calendar_events
             WHERE tenant_id = $1 AND id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(CalendarError::EventNotFound(id))?;
        tx.commit().await?;
        Ok(row)
    }


    /// List events in a calendar within optional time range, ordered by dtstart.
    pub async fn list(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        q: &EventQuery,
    ) -> Result<Vec<Event>> {
        let limit = q.limit.unwrap_or(1000).clamp(1, 10_000);
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query_as::<_, Event>(
            r#"
            SELECT id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                   description, location, dtstart, dtend, rrule, status,
                   sequence, organizer_email, created_at, updated_at
              FROM calendar_events
             WHERE tenant_id = $1
               AND calendar_id = $2
               AND ($3::timestamptz IS NULL OR dtend   IS NULL OR dtend   >= $3)
               AND ($4::timestamptz IS NULL OR dtstart IS NULL OR dtstart <= $4)
             ORDER BY dtstart NULLS LAST, created_at
             LIMIT $5
            "#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(q.from)
        .bind(q.to)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Update existing event by id, replacing ical_raw + derived fields.
    pub async fn update(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        raw: &str,
    ) -> Result<Event> {
        let parsed = ical::parse_vevent(raw)?;
        let etag   = ical::compute_etag(raw);

        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Event>(
            r#"
            UPDATE calendar_events SET
                uid             = $3,
                etag            = $4,
                ical_raw        = $5,
                summary         = $6,
                description     = $7,
                location        = $8,
                dtstart         = $9,
                dtend           = $10,
                rrule           = $11,
                status          = $12,
                organizer_email = $13,
                sequence        = CASE
                    WHEN summary         IS DISTINCT FROM $6
                      OR location        IS DISTINCT FROM $8
                      OR dtstart         IS DISTINCT FROM $9
                      OR dtend           IS DISTINCT FROM $10
                      OR rrule           IS DISTINCT FROM $11
                      OR status          IS DISTINCT FROM $12
                      OR organizer_email IS DISTINCT FROM $13
                    THEN sequence + 1
                    ELSE sequence
                END
             WHERE tenant_id = $1 AND id = $2
             RETURNING id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                       description, location, dtstart, dtend, rrule, status,
                       sequence, organizer_email, created_at, updated_at
            "#,
        )
        .bind(tenant_id)
        .bind(id)
        .bind(&parsed.uid)
        .bind(&etag)
        .bind(raw)
        .bind(&parsed.summary)
        .bind(&parsed.description)
        .bind(&parsed.location)
        .bind(parsed.dtstart)
        .bind(parsed.dtend)
        .bind(&parsed.rrule)
        .bind(&parsed.status)
        .bind(&parsed.organizer_email)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(CalendarError::EventNotFound(id))?;
        tx.commit().await?;
        Ok(row)
    }

    /// Delete event by id.
    pub async fn delete(&self, tenant_id: Uuid, id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"DELETE FROM calendar_events WHERE tenant_id = $1 AND id = $2"#,
        )
        .bind(tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if res.rows_affected() == 0 {
            return Err(CalendarError::EventNotFound(id));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Bulk-delete events whose `dtstart` cai em `[from, to)`. Retorna a contagem
    /// de linhas removidas. Eventos sem `dtstart` ficam de fora (sem timestamp pra
    /// posicionar no range). Útil pra sweep de eventos antigos ou cleanup em massa
    /// de calendário poluído (ex: import duplicado). Sprint #457.
    pub async fn delete_range(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"DELETE FROM calendar_events
                WHERE tenant_id   = $1
                  AND calendar_id = $2
                  AND dtstart IS NOT NULL
                  AND dtstart >= $3
                  AND dtstart <  $4"#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected())
    }

    /// Bulk-move events whose `dtstart` cai em `[from, to)` no calendar `src`
    /// para o calendar `dst` (mesmo tenant). Retorna a contagem de linhas
    /// movidas. Eventos sem `dtstart` ficam de fora (mesmo critério do
    /// `delete_range` #457). Single UPDATE muda só `calendar_id` — `ical_raw`
    /// não é re-parseado porque `calendar_id` é metadata externa ao VCALENDAR
    /// (RFC 5545 não tem propriedade calendar-id; CalDAV usa o path do
    /// collection pra associar). `etag` e `sequence` PRESERVADOS — semantically
    /// não houve mudança no conteúdo iCalendar (UID/SUMMARY/DTSTART/RRULE
    /// idênticos), só de "onde" o evento mora; UI/clients que cacheiam por
    /// ETag não precisam invalidar. `updated_at` preservado pelo mesmo motivo
    /// (timestamp de "última edição do conteúdo", não de "última edição da
    /// localização"). `(calendar_id, uid)` é UNIQUE — se UID colidir no `dst`,
    /// UPDATE falha com `unique_violation` que é convertido pra `Conflict` no
    /// handler. Sprint #546.
    pub async fn move_range(
        &self,
        tenant_id: Uuid,
        src: Uuid,
        dst: Uuid,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"UPDATE calendar_events
                  SET calendar_id = $3
                WHERE tenant_id   = $1
                  AND calendar_id = $2
                  AND dtstart IS NOT NULL
                  AND ($4::timestamptz IS NULL OR dtstart >= $4)
                  AND ($5::timestamptz IS NULL OR dtstart <  $5)"#,
        )
        .bind(tenant_id)
        .bind(src)
        .bind(dst)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected())
    }

    /// Bulk-set `status` em todos os eventos cujo `dtstart` ∈ `[from, to)`
    /// no `calendar_id` (single-tenant). Retorna a contagem de linhas
    /// afetadas. Eventos sem `dtstart` ficam de fora (mesmo critério do
    /// `delete_range` #457 / `move_range` #546). Single UPDATE muda só a
    /// coluna `status` — `ical_raw` NÃO é re-parseado mesmo o `STATUS`
    /// sendo propriedade VCALENDAR válida (RFC 5545 §3.8.1.11), porque
    /// re-parsear N raws + reserializar seria O(N) string work no loop
    /// equivalente a N `EventRepo::update`; trade-off explícito: a
    /// coluna `status` fica autoritativa (consultas SQL veem o valor
    /// novo), o `ical_raw` mantém o STATUS antigo até próximo PUT/UPDATE
    /// do cliente. CalDAV clients que leem via `ical_raw` (export ICS,
    /// download VCAL) verão valor stale; clientes que leem via API
    /// estruturada (GET /events) verão valor fresh. Documentação do
    /// endpoint deixa explícito. `etag`/`sequence`/`updated_at`
    /// preservados — mesma justificativa do #546 (mudança no metadata
    /// indexado, não no conteúdo iCalendar bruto). Sprint #547.
    pub async fn set_status_range(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        status: &str,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"UPDATE calendar_events
                  SET status = $3
                WHERE tenant_id   = $1
                  AND calendar_id = $2
                  AND dtstart IS NOT NULL
                  AND ($4::timestamptz IS NULL OR dtstart >= $4)
                  AND ($5::timestamptz IS NULL OR dtstart <  $5)"#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(status)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected())
    }

    /// UPSERT event by UID (CalDAV PUT semantics: idempotent per RFC 4791).
    pub async fn replace_by_uid(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        raw: &str,
    ) -> Result<Event> {
        let parsed = ical::parse_vevent(raw)?;
        let etag   = ical::compute_etag(raw);

        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Event>(
            r#"
            INSERT INTO calendar_events
                (tenant_id, calendar_id, uid, etag, ical_raw, summary, description,
                 location, dtstart, dtend, rrule, status, sequence, organizer_email)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            ON CONFLICT (calendar_id, uid) DO UPDATE SET
                etag            = EXCLUDED.etag,
                ical_raw        = EXCLUDED.ical_raw,
                summary         = EXCLUDED.summary,
                description     = EXCLUDED.description,
                location        = EXCLUDED.location,
                dtstart         = EXCLUDED.dtstart,
                dtend           = EXCLUDED.dtend,
                rrule           = EXCLUDED.rrule,
                status          = EXCLUDED.status,
                organizer_email = EXCLUDED.organizer_email,
                sequence        = CASE
                    WHEN calendar_events.summary         IS DISTINCT FROM EXCLUDED.summary
                      OR calendar_events.location        IS DISTINCT FROM EXCLUDED.location
                      OR calendar_events.dtstart         IS DISTINCT FROM EXCLUDED.dtstart
                      OR calendar_events.dtend           IS DISTINCT FROM EXCLUDED.dtend
                      OR calendar_events.rrule           IS DISTINCT FROM EXCLUDED.rrule
                      OR calendar_events.status          IS DISTINCT FROM EXCLUDED.status
                      OR calendar_events.organizer_email IS DISTINCT FROM EXCLUDED.organizer_email
                    THEN calendar_events.sequence + 1
                    ELSE calendar_events.sequence
                END
            RETURNING id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                      description, location, dtstart, dtend, rrule, status,
                      sequence, organizer_email, created_at, updated_at
            "#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(&parsed.uid)
        .bind(&etag)
        .bind(raw)
        .bind(&parsed.summary)
        .bind(&parsed.description)
        .bind(&parsed.location)
        .bind(parsed.dtstart)
        .bind(parsed.dtend)
        .bind(&parsed.rrule)
        .bind(&parsed.status)
        .bind(parsed.sequence)
        .bind(&parsed.organizer_email)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Fetch event by (calendar_id, uid) — CalDAV URI mapping.
    pub async fn get_by_uid(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        uid: &str,
    ) -> Result<Event> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Event>(
            r#"
            SELECT id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                   description, location, dtstart, dtend, rrule, status,
                   sequence, organizer_email, created_at, updated_at
              FROM calendar_events
             WHERE tenant_id = $1 AND calendar_id = $2 AND uid = $3
            "#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(uid)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| CalendarError::BadRequest(format!("event uid not found: {uid}")))?;
        tx.commit().await?;
        Ok(row)
    }

    /// Locate an event by UID across ALL calendars in the tenant.
    /// Used by iMIP REPLY ingestion: the responder may belong to any
    /// calendar owned by the tenant; UID is globally unique per RFC 5545.
    pub async fn find_by_uid_in_tenant(
        &self,
        tenant_id: Uuid,
        uid: &str,
    ) -> Result<Option<Event>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Event>(
            r#"
            SELECT id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                   description, location, dtstart, dtend, rrule, status,
                   sequence, organizer_email, created_at, updated_at
              FROM calendar_events
             WHERE tenant_id = $1 AND uid = $2
             LIMIT 1
            "#,
        )
        .bind(tenant_id)
        .bind(uid)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Fetch multiple events by UIDs (CalDAV calendar-multiget REPORT).
    pub async fn list_by_uids(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        uids: &[String],
    ) -> Result<Vec<Event>> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query_as::<_, Event>(
            r#"
            SELECT id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                   description, location, dtstart, dtend, rrule, status,
                   sequence, organizer_email, created_at, updated_at
              FROM calendar_events
             WHERE tenant_id = $1 AND calendar_id = $2 AND uid = ANY($3)
            "#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(uids)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Delete by (calendar_id, uid) — CalDAV DELETE on event URI.
    pub async fn delete_by_uid(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        uid: &str,
    ) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"DELETE FROM calendar_events
                WHERE tenant_id = $1 AND calendar_id = $2 AND uid = $3"#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(uid)
        .execute(&mut *tx)
        .await?;
        if res.rows_affected() == 0 {
            return Err(CalendarError::BadRequest(format!("event uid not found: {uid}")));
        }
        tx.commit().await?;
        Ok(())
    }

}

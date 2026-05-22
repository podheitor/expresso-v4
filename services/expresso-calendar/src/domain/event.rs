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
    pub class:           Option<String>,
    pub transp:          Option<String>,
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
                 location, dtstart, dtend, rrule, status, class, transp,
                 sequence, organizer_email)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
            RETURNING id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                      description, location, dtstart, dtend, rrule, status,
                      class, transp, sequence, organizer_email, created_at, updated_at
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
        .bind(&parsed.class)
        .bind(&parsed.transp)
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
                   class, transp, sequence, organizer_email, created_at, updated_at
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
                   class, transp, sequence, organizer_email, created_at, updated_at
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
                class           = $13,
                transp          = $14,
                organizer_email = $15,
                sequence        = CASE
                    WHEN summary         IS DISTINCT FROM $6
                      OR location        IS DISTINCT FROM $8
                      OR dtstart         IS DISTINCT FROM $9
                      OR dtend           IS DISTINCT FROM $10
                      OR rrule           IS DISTINCT FROM $11
                      OR status          IS DISTINCT FROM $12
                      OR class           IS DISTINCT FROM $13
                      OR transp          IS DISTINCT FROM $14
                      OR organizer_email IS DISTINCT FROM $15
                    THEN sequence + 1
                    ELSE sequence
                END
             WHERE tenant_id = $1 AND id = $2
             RETURNING id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                       description, location, dtstart, dtend, rrule, status,
                       class, transp, sequence, organizer_email, created_at, updated_at
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
        .bind(&parsed.class)
        .bind(&parsed.transp)
        .bind(&parsed.organizer_email)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(CalendarError::EventNotFound(id))?;

        sqlx::query(
            "INSERT INTO calendar_event_history \
             (tenant_id, calendar_id, event_id, etag, sequence, op) \
             VALUES ($1, $2, $3, $4, $5, 'PUT')",
        )
        .bind(tenant_id)
        .bind(row.calendar_id)
        .bind(row.id)
        .bind(&row.etag)
        .bind(row.sequence)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(row)
    }

    /// Patch individual fields on an event without replacing the full iCal body.
    /// `None` fields are preserved; `Some(None)` clears a nullable field.
    /// `ical_raw` is left STALE (structural authority remains with PUT).
    /// `etag` and `updated_at` are refreshed; `sequence` increments only when
    /// any of summary/location/dtstart/dtend/status changes.
    pub async fn patch_fields(
        &self,
        tenant_id:   Uuid,
        id:          Uuid,
        summary:     Option<Option<String>>,
        location:    Option<Option<String>>,
        description: Option<Option<String>>,
        dtstart:     Option<Option<OffsetDateTime>>,
        dtend:       Option<Option<OffsetDateTime>>,
        status:      Option<Option<String>>,
    ) -> Result<Event> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;

        let row = sqlx::query_as::<_, Event>(
            r#"
            UPDATE calendar_events SET
                summary     = CASE WHEN $3  THEN $4::TEXT    ELSE summary     END,
                location    = CASE WHEN $5  THEN $6::TEXT    ELSE location    END,
                description = CASE WHEN $7  THEN $8::TEXT    ELSE description END,
                dtstart     = CASE WHEN $9  THEN $10::TIMESTAMPTZ ELSE dtstart END,
                dtend       = CASE WHEN $11 THEN $12::TIMESTAMPTZ ELSE dtend   END,
                status      = CASE WHEN $13 THEN $14::TEXT   ELSE status      END,
                etag        = md5(gen_random_uuid()::text),
                sequence    = CASE
                    WHEN ($3  AND summary  IS DISTINCT FROM $4::TEXT)
                      OR ($5  AND location IS DISTINCT FROM $6::TEXT)
                      OR ($9  AND dtstart  IS DISTINCT FROM $10::TIMESTAMPTZ)
                      OR ($11 AND dtend    IS DISTINCT FROM $12::TIMESTAMPTZ)
                      OR ($13 AND status   IS DISTINCT FROM $14::TEXT)
                    THEN sequence + 1
                    ELSE sequence
                END
            WHERE tenant_id = $1 AND id = $2
            RETURNING id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                      description, location, dtstart, dtend, rrule, status,
                      class, transp, sequence, organizer_email, created_at, updated_at
            "#,
        )
        .bind(tenant_id)
        .bind(id)
        .bind(summary.is_some())
        .bind(summary.and_then(|v| v))
        .bind(location.is_some())
        .bind(location.and_then(|v| v))
        .bind(description.is_some())
        .bind(description.and_then(|v| v))
        .bind(dtstart.is_some())
        .bind(dtstart.and_then(|v| v))
        .bind(dtend.is_some())
        .bind(dtend.and_then(|v| v))
        .bind(status.is_some())
        .bind(status.and_then(|v| v))
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(CalendarError::EventNotFound(id))?;

        sqlx::query(
            "INSERT INTO calendar_event_history \
             (tenant_id, calendar_id, event_id, etag, sequence, op) \
             VALUES ($1, $2, $3, $4, $5, 'PATCH')",
        )
        .bind(tenant_id)
        .bind(row.calendar_id)
        .bind(row.id)
        .bind(&row.etag)
        .bind(row.sequence)
        .execute(&mut *tx)
        .await?;

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

    /// Bulk-set `class` (RFC 5545 §3.8.1.3) em todos os eventos cujo
    /// `dtstart` ∈ `[from, to)` no `calendar_id`. Sprint #555: paralelo
    /// direto do `set_status_range` #547 mas na coluna `class` adicionada
    /// na migração `20260805000000_calendar_event_class.sql` com CHECK
    /// enum `(PUBLIC, PRIVATE, CONFIDENTIAL)`. Mesmo trade-off do #547:
    /// `ical_raw` NÃO é re-parseado — coluna `class` fica autoritativa,
    /// `CLASS:` dentro do raw fica STALE até próximo PUT. `etag`/
    /// `sequence`/`updated_at` preservados.
    pub async fn set_class_range(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        class: &str,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"UPDATE calendar_events
                  SET class = $3
                WHERE tenant_id   = $1
                  AND calendar_id = $2
                  AND dtstart IS NOT NULL
                  AND ($4::timestamptz IS NULL OR dtstart >= $4)
                  AND ($5::timestamptz IS NULL OR dtstart <  $5)"#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(class)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected())
    }

    /// Bulk-set `transp` (RFC 5545 §3.8.2.7 TRANSP) em todos os eventos cujo
    /// `dtstart` ∈ `[from, to)` no `calendar_id`. Sprint #556: paralelo direto
    /// do `set_class_range` #555 mas na coluna `transp` adicionada na migração
    /// `20260806000000_calendar_event_transp.sql` com CHECK enum
    /// `(OPAQUE, TRANSPARENT)`. Mesmo trade-off cross-channel: `ical_raw` NÃO
    /// é re-parseado, coluna `transp` fica autoritativa, `TRANSP:` dentro do
    /// raw fica STALE até próximo PUT. Free/busy #460 ainda NÃO consulta
    /// `transp` — sprint futuro deve filtrar `WHERE transp IS DISTINCT FROM
    /// 'TRANSPARENT'` pra excluir eventos transparentes do bloco.
    pub async fn set_transparency_range(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        transp: &str,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"UPDATE calendar_events
                  SET transp = $3
                WHERE tenant_id   = $1
                  AND calendar_id = $2
                  AND dtstart IS NOT NULL
                  AND ($4::timestamptz IS NULL OR dtstart >= $4)
                  AND ($5::timestamptz IS NULL OR dtstart <  $5)"#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(transp)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected())
    }

    /// Bulk-clear `rrule` (set NULL) em todos os eventos cujo `dtstart` ∈
    /// `[from, to)` no `calendar_id` (single-tenant). Retorna a contagem de
    /// linhas afetadas (incluindo eventos que já tinham `rrule = NULL` por
    /// estarem dentro do range — UPDATE não filtra por valor prévio; UI que
    /// quer "só os que tinham rrule" pode primeiro chamar `events-by-range`
    /// #544 com `?with_rrule=true` ou olhar `by_recurrence` em #545).
    /// Variante simplificada do `set-rrule` futuro: NULL é o valor unário
    /// trivial, sem validação RRULE. Mesmo trade-off do #547 (`set_status_range`):
    /// `ical_raw` NÃO é re-parseado, então a coluna `rrule` (autoritativa
    /// pra GET estruturado e queries SQL como #464/#469) fica fresh, mas
    /// a propriedade `RRULE:` dentro do `ical_raw` permanece STALE até
    /// próximo PUT/UPDATE do cliente — clientes CalDAV puros que parseiam
    /// o raw verão o evento como recorrente até reescrita. Decisão paralela
    /// ao #547 e justificada pelo mesmo motivo: re-parsear N raws é O(N)
    /// string work proibitivo em massa. `etag`/`sequence`/`updated_at`
    /// preservados (paralelo do #546/#547). Diferente do `set-rrule` (futuro)
    /// que precisaria validar o valor RRULE antes do UPDATE — `clear` é o
    /// único caso onde nenhuma validação extra cabe. Sprint #548.
    pub async fn clear_rrule_range(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"UPDATE calendar_events
                  SET rrule = NULL
                WHERE tenant_id   = $1
                  AND calendar_id = $2
                  AND dtstart IS NOT NULL
                  AND ($3::timestamptz IS NULL OR dtstart >= $3)
                  AND ($4::timestamptz IS NULL OR dtstart <  $4)"#,
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

    /// Bulk-set `summary` em todos os eventos cujo `dtstart` ∈ `[from, to)`
    /// no `calendar_id` (single-tenant). Retorna a contagem de linhas
    /// afetadas. Eventos sem `dtstart` ficam de fora (mesmo critério do
    /// `delete_range` #457 / `move_range` #546 / `set_status_range` #547 /
    /// `clear_rrule_range` #548). Single UPDATE muda só a coluna `summary`.
    /// Validação no handler: `summary` trim'd não vazio (paralelo do
    /// `EventRepo::update` que aceita summary não-vazio) — DDL não tem
    /// NOT NULL no `summary` (pode ser NULL via PUT sem SUMMARY no VCAL),
    /// mas bulk-set pra string vazia é provavelmente erro de UI; aceitar
    /// NULL via flag `?clear=true` em sprint futuro se demanda surgir.
    /// Mesmo trade-off do #547/#548: `ical_raw` NÃO é re-parseado, então
    /// a coluna `summary` (autoritativa pra GET estruturado da API) fica
    /// fresh, mas o `SUMMARY:` dentro do `ical_raw` permanece STALE até
    /// próximo PUT/UPDATE — clientes CalDAV que parseiam o raw verão
    /// summary antigo (export ICS, download VCAL). `etag`/`sequence`/
    /// `updated_at` PRESERVADOS pelo mesmo motivo do #546/#547/#548.
    /// Sprint #549.
    pub async fn set_summary_range(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        summary: &str,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"UPDATE calendar_events
                  SET summary = $3
                WHERE tenant_id   = $1
                  AND calendar_id = $2
                  AND dtstart IS NOT NULL
                  AND ($4::timestamptz IS NULL OR dtstart >= $4)
                  AND ($5::timestamptz IS NULL OR dtstart <  $5)"#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(summary)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected())
    }

    /// Bulk-set `location` em todos os eventos cujo `dtstart` ∈ `[from, to)`
    /// no `calendar_id` (single-tenant). Retorna a contagem de linhas
    /// afetadas. Eventos sem `dtstart` ficam de fora (mesmo critério do
    /// `delete_range` #457 / `move_range` #546 / `set_status_range` #547 /
    /// `clear_rrule_range` #548 / `set_summary_range` #549). Single UPDATE
    /// muda só a coluna `location`. Diferente do `set_summary_range` #549,
    /// `location` é `Option<&str>` — `None` ⇒ `SET location = NULL` (clear
    /// semantics), `Some(s)` ⇒ `SET location = $3`. Decisão de política:
    /// DDL `location` é nullable (RFC 5545 §3.8.1.7 LOCATION é opcional em
    /// VEVENT), portanto empty/whitespace via API é tratado como "clear"
    /// (handler converte `?location=` ou whitespace-only pra `None`) em
    /// vez de rejeitar com 400 como #549 fez pro summary. Mesmo trade-off
    /// dos #547/#548/#549: `ical_raw` NÃO é re-parseado, então a coluna
    /// `location` (autoritativa pra GET estruturado da API) fica fresh, mas
    /// o `LOCATION:` dentro do `ical_raw` permanece STALE até próximo
    /// PUT/UPDATE. `etag`/`sequence`/`updated_at` PRESERVADOS pelo mesmo
    /// motivo do #546/#547/#548/#549. Sprint #550.
    pub async fn set_location_range(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        location: Option<&str>,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"UPDATE calendar_events
                  SET location = $3
                WHERE tenant_id   = $1
                  AND calendar_id = $2
                  AND dtstart IS NOT NULL
                  AND ($4::timestamptz IS NULL OR dtstart >= $4)
                  AND ($5::timestamptz IS NULL OR dtstart <  $5)"#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(location)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected())
    }

    /// Bulk-set `description` em todos os eventos cujo `dtstart` ∈ `[from, to)`
    /// no `calendar_id` (single-tenant). Retorna a contagem de linhas
    /// afetadas. Eventos sem `dtstart` ficam de fora (mesmo critério do
    /// `delete_range` #457 / `move_range` #546 / `set_status_range` #547 /
    /// `clear_rrule_range` #548 / `set_summary_range` #549 /
    /// `set_location_range` #550). Single UPDATE muda só a coluna
    /// `description`. Paralelo DIRETO do `set_location_range` #550 (não do
    /// #549) — assinatura `description: Option<&str>` em vez de `&str`
    /// porque DDL `description` é nullable e RFC 5545 §3.8.1.5 DESCRIPTION
    /// é opcional em VEVENT (mesma justificativa do #550 location): `None`
    /// ⇒ `SET description = NULL` (clear), `Some(s)` ⇒ `SET description =
    /// $3` (set não-null). Mesmo trade-off filosófico do #547/#548/#549/
    /// #550: `ical_raw` NÃO é re-parseado, então a coluna `description`
    /// (autoritativa pra GET estruturado da API e search FTS via #461)
    /// fica fresh, mas o `DESCRIPTION:` dentro do `ical_raw` permanece
    /// STALE até próximo PUT/UPDATE. `etag`/`sequence`/`updated_at`
    /// PRESERVADOS pelo mesmo motivo do #546/#547/#548/#549/#550. Sprint
    /// #551.
    pub async fn set_description_range(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        description: Option<&str>,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"UPDATE calendar_events
                  SET description = $3
                WHERE tenant_id   = $1
                  AND calendar_id = $2
                  AND dtstart IS NOT NULL
                  AND ($4::timestamptz IS NULL OR dtstart >= $4)
                  AND ($5::timestamptz IS NULL OR dtstart <  $5)"#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(description)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected())
    }

    /// Bulk-set `organizer_email` em todos os eventos cujo `dtstart` ∈
    /// `[from, to)` no `calendar_id` (single-tenant). Retorna a contagem de
    /// linhas afetadas. Eventos sem `dtstart` ficam de fora (mesmo critério
    /// do `delete_range` #457 / `move_range` #546 / `set_status_range` #547 /
    /// `clear_rrule_range` #548 / `set_summary_range` #549 /
    /// `set_location_range` #550 / `set_description_range` #551). Single
    /// UPDATE muda só a coluna `organizer_email`. Paralelo DIRETO do
    /// `set_location_range` #550 / `set_description_range` #551 — assinatura
    /// `organizer: Option<&str>` em vez de `&str` porque DDL
    /// `organizer_email TEXT` é nullable (sem NOT NULL) e RFC 5545 §3.8.4.3
    /// ORGANIZER é opcional em VEVENT (`None` ⇒ `SET organizer_email = NULL`
    /// clear, `Some(s)` ⇒ set não-null). DIFERENÇA SEMÂNTICA vs #549/#550/
    /// #551: organizer é metadata de ATRIBUIÇÃO/PROPRIEDADE (não TEXT-livre
    /// descritivo) — em fluxos iTIP (#491) o `organizer_email` é o remetente
    /// das mensagens REQUEST/CANCEL/REPLY, e mudar em massa pode reescrever
    /// "quem convidou" pra eventos já comunicados externamente. AINDA ASSIM
    /// ACEITO como simples bulk-set sem disparar iTIP outbound — UI/admin
    /// é responsável por entender que NÃO há re-envio automático de REQUEST
    /// (paralelo do trade-off `ical_raw stale` mas no eixo de protocolo
    /// externo em vez de armazenamento interno). Mesmo trade-off filosófico
    /// do #547/#548/#549/#550/#551: `ical_raw` NÃO é re-parseado, então a
    /// coluna `organizer_email` (autoritativa pro GET estruturado da API e
    /// pro pipeline iMIP outbound subsequente do #491) fica fresh, mas o
    /// `ORGANIZER:mailto:...` dentro do `ical_raw` permanece STALE até
    /// próximo PUT/UPDATE. `etag`/`sequence`/`updated_at` PRESERVADOS pelo
    /// mesmo motivo do #546/#547/#548/#549/#550/#551 — clientes CalDAV não
    /// veem mudança via If-Match/sync-token, evita storm de re-syncs. Sprint
    /// #552.
    pub async fn set_organizer_email_range(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        organizer: Option<&str>,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"UPDATE calendar_events
                  SET organizer_email = $3
                WHERE tenant_id   = $1
                  AND calendar_id = $2
                  AND dtstart IS NOT NULL
                  AND ($4::timestamptz IS NULL OR dtstart >= $4)
                  AND ($5::timestamptz IS NULL OR dtstart <  $5)"#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(organizer)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected())
    }

    /// Bulk-set `rrule` em todos os eventos cujo `dtstart` ∈ `[from, to)` no
    /// `calendar_id` (single-tenant). Retorna a contagem de linhas afetadas.
    /// Eventos sem `dtstart` ficam de fora (mesmo critério do `delete_range`
    /// #457 / `move_range` #546 / `set_status_range` #547 / `clear_rrule_range`
    /// #548 / `set_summary_range` #549 / `set_location_range` #550 /
    /// `set_description_range` #551 / `set_organizer_email_range` #552).
    /// Single UPDATE muda só a coluna `rrule`. Paralelo do
    /// `set_organizer_email_range` #552 na assinatura `Option<&str>` (DDL
    /// `rrule TEXT` é nullable e RFC 5545 §3.8.5.3 RRULE é opcional em VEVENT
    /// — `None` ⇒ NULL clear), mas com VALIDAÇÃO server-side da string RRULE
    /// **NÃO feita aqui** (este método repo é puramente storage; validação
    /// fica na camada API que chama `crate::domain::rrule::Rrule::parse(s)`
    /// antes de invocar este método — paralelo do `EventRepo::update` que
    /// confia no `ical::parse_vevent` upstream). CLASSE active-recurrence
    /// (insight #3 do sprint #552): mudar RRULE em massa RE-EXPANDE a série
    /// virtualmente — endpoints `events-recurrence-stats` #464,
    /// `events-recurrence-monthly` #469, `events/:id/instances` #500 e
    /// `events-by-range/range-instances` #501 retornam outros conjuntos
    /// imediatamente sem passar por re-render explícito (computa instances
    /// on-demand a partir da coluna `rrule`). Mesmo trade-off filosófico do
    /// #547-#552: `ical_raw` NÃO é re-parseado, então a coluna `rrule`
    /// (autoritativa pra recurrence-stats #464, recurrence-monthly #469,
    /// instances #500, range-instances #501 e expansão free/busy) fica
    /// fresh, mas `RRULE:` dentro do `ical_raw` permanece STALE até próximo
    /// PUT/UPDATE — clientes CalDAV puros parseando o raw verão FREQ antiga.
    /// EXDATEs e overrides relacionados à série antiga ficam dangling: um
    /// EXDATE pra instance-date X que existe na expansion da rrule antiga
    /// mas NÃO na nova fica armazenado mas sem efeito (se recreado pela nova
    /// rrule, volta a suprimir; senão, dormente). Mesmo pra overrides com
    /// RECURRENCE-ID — se o RECURRENCE-ID não for produzido pela nova rrule,
    /// o override fica órfão (existe no DB mas nunca é apresentado na
    /// expansion). UI/admin que quer limpeza explícita deve chamar
    /// `clear_exdates`/listar overrides após o set-rrule. `etag`/`sequence`/
    /// `updated_at` PRESERVADOS (paralelo do #546-#552). Sprint #553.
    pub async fn set_rrule_range(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        rrule: Option<&str>,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"UPDATE calendar_events
                  SET rrule = $3
                WHERE tenant_id   = $1
                  AND calendar_id = $2
                  AND dtstart IS NOT NULL
                  AND ($4::timestamptz IS NULL OR dtstart >= $4)
                  AND ($5::timestamptz IS NULL OR dtstart <  $5)"#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(rrule)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected())
    }

    /// Bulk-set multi-coluna TEXT em todos eventos cujo `dtstart` ∈ `[from, to)`
    /// no `calendar_id` (single-tenant). Cada parâmetro é tri-state: `None`
    /// preserva a coluna; `Some(None)` clear (NULL); `Some(Some(s))` seta. SQL
    /// usa `CASE WHEN $flag THEN $value ELSE column END` por coluna pra suportar
    /// os 3 estados num único UPDATE atômico — mais ergonômico pra UI "Edit all
    /// events in date range" com form multi-field que o cliente sequencial de 4
    /// chamadas a `set_summary_range` / `set_location_range` /
    /// `set_description_range` / `set_organizer_email_range`. Sprint #554:
    /// consolidação dos 4 sprints individuais (#549/#550/#551/#552) num único
    /// endpoint que retém a flexibilidade single-field via tri-state em cada
    /// param. Eventos sem `dtstart` ficam de fora (mesmo critério dos outros
    /// bulk-set). Validação dos valores (não-vazia pra summary, formato
    /// pra organizer_email se desejado) fica na camada API. Rrule INTENCIONAL-
    /// MENTE NÃO incluído porque sua mutação tem semantics ativa de recurrence
    /// (insight #4 do #553) e não passive-display como as 4 colunas TEXT-livre
    /// aqui — manter set-rrule como endpoint separado preserva a separação de
    /// classes (active-recurrence vs passive-display). Mesmo trade-off interno
    /// do #547-#553: `ical_raw` permanece STALE até próximo PUT/UPDATE
    /// regular — search FTS #461 fica STALE pra os summaries/descriptions/
    /// locations atualizadas (paralelo do #549-#551). Retorna a contagem de
    /// linhas afetadas. NO-OP semantic preservada: chamar com TODOS os 4
    /// params None retorna 0 sem tocar DB (validado upstream — repo não checa
    /// pra manter atomicidade do UPDATE).
    #[allow(clippy::too_many_arguments)]
    pub async fn set_text_fields_range(
        &self,
        tenant_id: Uuid,
        calendar_id: Uuid,
        summary:        Option<Option<&str>>,
        location:       Option<Option<&str>>,
        description:    Option<Option<&str>>,
        organizer_email: Option<Option<&str>>,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"UPDATE calendar_events
                  SET summary         = CASE WHEN $3  THEN $4  ELSE summary         END,
                      location        = CASE WHEN $5  THEN $6  ELSE location        END,
                      description     = CASE WHEN $7  THEN $8  ELSE description     END,
                      organizer_email = CASE WHEN $9  THEN $10 ELSE organizer_email END
                WHERE tenant_id   = $1
                  AND calendar_id = $2
                  AND dtstart IS NOT NULL
                  AND ($11::timestamptz IS NULL OR dtstart >= $11)
                  AND ($12::timestamptz IS NULL OR dtstart <  $12)"#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(summary.is_some())
        .bind(summary.flatten())
        .bind(location.is_some())
        .bind(location.flatten())
        .bind(description.is_some())
        .bind(description.flatten())
        .bind(organizer_email.is_some())
        .bind(organizer_email.flatten())
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
                 location, dtstart, dtend, rrule, status, class, transp,
                 sequence, organizer_email)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
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
                class           = EXCLUDED.class,
                transp          = EXCLUDED.transp,
                organizer_email = EXCLUDED.organizer_email,
                sequence        = CASE
                    WHEN calendar_events.summary         IS DISTINCT FROM EXCLUDED.summary
                      OR calendar_events.location        IS DISTINCT FROM EXCLUDED.location
                      OR calendar_events.dtstart         IS DISTINCT FROM EXCLUDED.dtstart
                      OR calendar_events.dtend           IS DISTINCT FROM EXCLUDED.dtend
                      OR calendar_events.rrule           IS DISTINCT FROM EXCLUDED.rrule
                      OR calendar_events.status          IS DISTINCT FROM EXCLUDED.status
                      OR calendar_events.class           IS DISTINCT FROM EXCLUDED.class
                      OR calendar_events.transp          IS DISTINCT FROM EXCLUDED.transp
                      OR calendar_events.organizer_email IS DISTINCT FROM EXCLUDED.organizer_email
                    THEN calendar_events.sequence + 1
                    ELSE calendar_events.sequence
                END
            RETURNING id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                      description, location, dtstart, dtend, rrule, status,
                      class, transp, sequence, organizer_email, created_at, updated_at
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
        .bind(&parsed.class)
        .bind(&parsed.transp)
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
                   class, transp, sequence, organizer_email, created_at, updated_at
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
                   class, transp, sequence, organizer_email, created_at, updated_at
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
                   class, transp, sequence, organizer_email, created_at, updated_at
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

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn sample() -> Event {
        Event {
            id:              Uuid::nil(),
            calendar_id:     Uuid::nil(),
            tenant_id:       Uuid::nil(),
            uid:             "uid-abc@example.com".into(),
            etag:            "etag123".into(),
            ical_raw:        "BEGIN:VCALENDAR\r\nEND:VCALENDAR".into(),
            summary:         Some("Team Meeting".into()),
            description:     None,
            location:        None,
            dtstart:         Some(datetime!(2026-06-01 09:00:00 UTC)),
            dtend:           Some(datetime!(2026-06-01 10:00:00 UTC)),
            rrule:           None,
            status:          Some("CONFIRMED".into()),
            class:           None,
            transp:          None,
            sequence:        0,
            organizer_email: Some("org@example.com".into()),
            created_at:      datetime!(2026-05-22 08:00:00 UTC),
            updated_at:      datetime!(2026-05-22 08:00:00 UTC),
        }
    }

    #[test]
    fn event_serde_roundtrip() {
        let e = sample();
        let s = serde_json::to_string(&e).unwrap();
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(back.uid, "uid-abc@example.com");
        assert_eq!(back.sequence, 0);
    }

    #[test]
    fn event_dtstart_in_rfc3339() {
        let e = sample();
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("2026-06-01T09:00:00"));
    }

    #[test]
    fn event_optional_fields_null_when_none() {
        let e = sample();
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert!(v["description"].is_null());
        assert!(v["rrule"].is_null());
    }

    #[test]
    fn event_query_default_all_none() {
        let q = EventQuery::default();
        assert!(q.from.is_none());
        assert!(q.to.is_none());
        assert!(q.limit.is_none());
    }

    #[test]
    fn event_query_deserializes_rfc3339() {
        let json = r#"{"from":"2026-06-01T00:00:00Z","to":"2026-06-30T23:59:59Z","limit":50}"#;
        let q: EventQuery = serde_json::from_str(json).unwrap();
        assert!(q.from.is_some());
        assert!(q.to.is_some());
        assert_eq!(q.limit, Some(50));
    }

    #[test]
    fn event_query_limit_none_by_default() {
        let q = EventQuery::default();
        assert!(q.limit.is_none());
    }

    #[test]
    fn event_query_to_without_from() {
        let json = r#"{"to":"2026-12-31T23:59:59Z"}"#;
        let q: EventQuery = serde_json::from_str(json).unwrap();
        assert!(q.from.is_none());
        assert!(q.to.is_some());
    }

    #[test]
    fn event_query_from_and_to_both_set() {
        let json = r#"{"from":"2026-01-01T00:00:00Z","to":"2026-12-31T23:59:59Z"}"#;
        let q: EventQuery = serde_json::from_str(json).unwrap();
        assert!(q.from.is_some());
        assert!(q.to.is_some());
    }

    #[test]
    fn event_query_from_without_to() {
        let json = r#"{"from":"2026-06-01T00:00:00Z"}"#;
        let q: EventQuery = serde_json::from_str(json).unwrap();
        assert!(q.from.is_some());
        assert!(q.to.is_none());
    }

    #[test]
    fn event_query_empty_is_all_none() {
        let q: EventQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.from.is_none());
        assert!(q.to.is_none());
    }

    #[test]
    fn event_query_limit_default_is_none() {
        let q: EventQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.limit.is_none());
    }

    #[test]
    fn event_query_limit_100_preserved() {
        let q: EventQuery = serde_json::from_str(r#"{"limit":100}"#).unwrap();
        assert_eq!(q.limit, Some(100));
    }

    #[test]
    fn event_query_limit_zero_preserved() {
        let q: EventQuery = serde_json::from_str(r#"{"limit":0}"#).unwrap();
        assert_eq!(q.limit, Some(0));
    }

    #[test]
    fn event_query_missing_limit_is_none() {
        let q: EventQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.limit.is_none());
    }

    #[test]
    fn event_query_limit_one_preserved() {
        let q: EventQuery = serde_json::from_str(r#"{"limit":1}"#).unwrap();
        assert_eq!(q.limit, Some(1));
    }

    #[test]
    fn event_query_limit_50_preserved() {
        let q: EventQuery = serde_json::from_str(r#"{"limit":50}"#).unwrap();
        assert_eq!(q.limit, Some(50));
    }
}

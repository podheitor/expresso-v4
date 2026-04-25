//! Free/busy lookup (RFC 6638 scheduling subset).
//!
//! Aggregates busy intervals across all calendars owned by a set of attendee
//! emails within a tenant. Cancelled events are excluded. RRULE expansion is
//! NOT performed here — only the master VEVENT dtstart/dtend are returned
//! (recurrence expansion is a separate follow-up; see ROADMAP Sprint 8-9).
//!
//! Tenant scoping: `lookup` abre transação via `begin_tenant_tx` para
//! defense-in-depth — o JOIN usa `WHERE e.tenant_id = $1 AND u.tenant_id = $1`
//! explícitos, e RLS de `calendar_events`/`calendars`/`users` filtra junto.

use std::collections::BTreeMap;

use expresso_core::{begin_tenant_tx, DbPool};
use serde::Serialize;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

/// Single busy window returned to callers.
#[derive(Debug, Clone, Serialize)]
pub struct BusyInterval {
    #[serde(with = "time::serde::rfc3339")]
    pub start: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub end:   OffsetDateTime,
}

#[derive(Debug, FromRow)]
struct BusyRow {
    email:   String,
    dtstart: OffsetDateTime,
    dtend:   Option<OffsetDateTime>,
    rrule:   Option<String>,
}

pub struct FreeBusyRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> FreeBusyRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Query busy intervals for the given attendee emails within [from, to].
    ///
    /// Returns a map keyed by the input email (lowercased). Attendees with no
    /// account or no events in range appear with an empty vector so callers
    /// can distinguish "not found" from "free".
    pub async fn lookup(
        &self,
        tenant_id: Uuid,
        attendees: &[String],
        from: OffsetDateTime,
        to:   OffsetDateTime,
    ) -> Result<BTreeMap<String, Vec<BusyInterval>>> {
        // Normalize inputs → lowercase, deduplicate, cap to avoid pathological
        // query sizes. Preserve original order for deterministic output when
        // caller iterates the result map.
        let lowered: Vec<String> = attendees
            .iter()
            .map(|a| a.trim().to_ascii_lowercase())
            .filter(|a| !a.is_empty())
            .collect();

        let mut out: BTreeMap<String, Vec<BusyInterval>> = BTreeMap::new();
        for a in &lowered {
            out.entry(a.clone()).or_default();
        }
        if lowered.is_empty() {
            return Ok(out);
        }

        // Join users → calendars → events; return per-email rows within range.
        // status filter: exclude CANCELLED; treat NULL status as busy.
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query_as::<_, BusyRow>(
            r#"
            SELECT lower(u.email) AS email,
                   e.dtstart      AS dtstart,
                   e.dtend        AS dtend,
                   e.rrule        AS rrule
              FROM calendar_events e
              JOIN calendars       c ON c.id            = e.calendar_id
              JOIN users           u ON u.id            = c.owner_user_id
             WHERE e.tenant_id  = $1
               AND u.tenant_id  = $1
               AND lower(u.email) = ANY($2)
               AND (e.status IS NULL OR e.status <> 'CANCELLED')
               AND e.dtstart IS NOT NULL
               AND e.dtstart <  $4
               AND (e.dtend IS NULL OR e.dtend > $3)
            "#,
        )
        .bind(tenant_id)
        .bind(&lowered)
        .bind(from)
        .bind(to)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        for r in rows {
            // Base duration from master VEVENT.
            let base_end = r.dtend.unwrap_or(r.dtstart);
            let duration = base_end - r.dtstart;

            // Try RRULE expansion; if rule missing or unparsable, fall back
            // to single-instance clamping. RRULE expander enforces its own
            // iteration cap (MAX_ITER=1000).
            let intervals: Vec<(time::OffsetDateTime, time::OffsetDateTime)> = match r.rrule.as_deref() {
                Some(raw) => match super::rrule::Rrule::parse(raw) {
                    Some(rule) => rule.expand(r.dtstart, duration, from, to),
                    None       => super::rrule::single_instance(r.dtstart, r.dtend, from, to)
                        .into_iter().collect(),
                },
                None => super::rrule::single_instance(r.dtstart, r.dtend, from, to)
                    .into_iter().collect(),
            };

            let bucket = out.entry(r.email).or_default();
            for (s, e) in intervals {
                // Final clamp (expander may emit slightly wider end).
                let start = if s < from { from } else { s };
                let end   = if e > to   { to }   else { e };
                if end > start {
                    bucket.push(BusyInterval { start, end });
                }
            }
        }

        // Sort each attendee's intervals by start for stable output.
        for v in out.values_mut() {
            v.sort_by_key(|b| b.start);
        }
        Ok(out)
    }
}

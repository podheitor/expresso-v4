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
    pub end: OffsetDateTime,
}

#[derive(Debug, FromRow)]
struct BusyRow {
    email: String,
    dtstart: OffsetDateTime,
    dtend: Option<OffsetDateTime>,
    rrule: Option<String>,
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
    ///
    /// `include_transparent`: when `false` (default), eventos com
    /// `transp = 'TRANSPARENT'` (RFC 5545 §3.8.2.7) são excluídos do busy set
    /// — RFC 4791 §7.10 mandata que TRANSPARENT seja "não-bloqueante" pra
    /// free/busy lookup. Quando `true`, eventos transparentes contam como
    /// busy (preserva comportamento pré-#557 pra clientes que dependiam dele).
    /// `transp = NULL` é tratado como OPAQUE (default RFC: bloqueia).
    pub async fn lookup(
        &self,
        tenant_id: Uuid,
        attendees: &[String],
        from: OffsetDateTime,
        to: OffsetDateTime,
        include_transparent: bool,
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
        // transp filter: when include_transparent=false (default), exclude
        // events explicitly marked TRANSPARENT (#556 column); NULL transp is
        // treated as OPAQUE (RFC default). Flag $5 short-circuits when caller
        // explicitly opts in to legacy "all events block" behaviour.
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
               AND ($5 OR e.transp IS DISTINCT FROM 'TRANSPARENT')
               AND e.dtstart IS NOT NULL
               AND e.dtstart <  $4
               AND (e.dtend IS NULL OR e.dtend > $3)
            "#,
        )
        .bind(tenant_id)
        .bind(&lowered)
        .bind(from)
        .bind(to)
        .bind(include_transparent)
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
            let intervals: Vec<(time::OffsetDateTime, time::OffsetDateTime)> =
                match r.rrule.as_deref() {
                    Some(raw) => match super::rrule::Rrule::parse(raw) {
                        Some(rule) => rule.expand(r.dtstart, duration, from, to),
                        None => super::rrule::single_instance(r.dtstart, r.dtend, from, to)
                            .into_iter()
                            .collect(),
                    },
                    None => super::rrule::single_instance(r.dtstart, r.dtend, from, to)
                        .into_iter()
                        .collect(),
                };

            let bucket = out.entry(r.email).or_default();
            for (s, e) in intervals {
                // Final clamp (expander may emit slightly wider end).
                let start = if s < from { from } else { s };
                let end = if e > to { to } else { e };
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

    /// Out-of-working-hours busy intervals per attendee email within [from, to].
    /// For each attendee that has working hours configured, every gap *outside*
    /// their windows is returned as a busy interval (so the scheduler avoids
    /// off-hours slots). Attendees with no working-hours rows are omitted (no
    /// constraint). Computed in UTC — per-user timezone conversion is a
    /// documented follow-up; the minute offsets are treated as UTC for now.
    pub async fn working_hours_busy(
        &self,
        tenant_id: Uuid,
        attendees: &[String],
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> Result<BTreeMap<String, Vec<BusyInterval>>> {
        let lowered: Vec<String> = attendees
            .iter()
            .map(|a| a.trim().to_ascii_lowercase())
            .filter(|a| !a.is_empty())
            .collect();
        let mut out: BTreeMap<String, Vec<BusyInterval>> = BTreeMap::new();
        if lowered.is_empty() {
            return Ok(out);
        }

        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows: Vec<WorkingHourRow> = sqlx::query_as(
            "SELECT lower(u.email) AS email, w.weekday, w.start_minute, w.end_minute \
               FROM user_working_hours w \
               JOIN users u ON u.id = w.user_id AND u.tenant_id = w.tenant_id \
              WHERE w.tenant_id = $1 AND lower(u.email) = ANY($2)",
        )
        .bind(tenant_id)
        .bind(&lowered)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        // Group windows per email per weekday.
        let mut windows: BTreeMap<String, [Vec<(i32, i32)>; 7]> = BTreeMap::new();
        for r in rows {
            let entry = windows.entry(r.email).or_default();
            let wd = (r.weekday.clamp(0, 6)) as usize;
            entry[wd].push((r.start_minute, r.end_minute));
        }

        for (email, by_day) in windows {
            let busy = off_hours_intervals(&by_day, from, to);
            if !busy.is_empty() {
                out.insert(email, busy);
            }
        }
        Ok(out)
    }
}

#[derive(Debug, FromRow)]
struct WorkingHourRow {
    email: String,
    weekday: i16,
    start_minute: i32,
    end_minute: i32,
}

/// Given per-weekday working windows (minutes from midnight, UTC), return the
/// busy intervals covering every off-hours gap within [from, to]. A weekday with
/// no windows is fully busy; a weekday with windows is busy in the gaps before /
/// between / after them. Bounded: iterates whole days across the (≤370-day)
/// freebusy window.
fn off_hours_intervals(
    by_day: &[Vec<(i32, i32)>; 7],
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Vec<BusyInterval> {
    use time::Duration;
    let mut out = Vec::new();
    // Walk day-by-day from the UTC midnight on/just before `from`.
    let mut day = from.replace_time(time::Time::MIDNIGHT);
    while day < to {
        let next_day = day + Duration::days(1);
        // Sunday=0..Saturday=6 to match the stored weekday convention.
        let wd = day.weekday().number_days_from_sunday() as usize;
        let mut windows = by_day[wd].clone();
        windows.sort_unstable();
        // Build busy gaps = day minus the union of windows.
        let mut cursor = 0i32; // minutes from midnight
        for (ws, we) in windows {
            if ws > cursor {
                push_clamped(&mut out, day, cursor, ws, from, to);
            }
            cursor = cursor.max(we);
        }
        if cursor < 1440 {
            push_clamped(&mut out, day, cursor, 1440, from, to);
        }
        day = next_day;
    }
    out
}

/// Push a busy interval for `day + [start_min, end_min)` clamped to [from, to].
fn push_clamped(
    out: &mut Vec<BusyInterval>,
    day: OffsetDateTime,
    start_min: i32,
    end_min: i32,
    from: OffsetDateTime,
    to: OffsetDateTime,
) {
    use time::Duration;
    let s = (day + Duration::minutes(i64::from(start_min))).max(from);
    let e = (day + Duration::minutes(i64::from(end_min))).min(to);
    if e > s {
        out.push(BusyInterval { start: s, end: e });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn empty_week() -> [Vec<(i32, i32)>; 7] {
        Default::default()
    }

    #[test]
    fn no_windows_means_whole_span_busy() {
        // 2026-06-01 is a Monday; one full UTC day, no windows → fully busy.
        let from = datetime!(2026-06-01 00:00:00 UTC);
        let to = datetime!(2026-06-02 00:00:00 UTC);
        let busy = off_hours_intervals(&empty_week(), from, to);
        assert_eq!(busy.len(), 1);
        assert_eq!(busy[0].start, from);
        assert_eq!(busy[0].end, to);
    }

    #[test]
    fn window_splits_day_into_before_and_after() {
        // Monday 09:00-17:00 working → busy [00:00,09:00) and [17:00,24:00).
        let mut wk = empty_week();
        wk[1] = vec![(9 * 60, 17 * 60)]; // Monday=1
        let from = datetime!(2026-06-01 00:00:00 UTC);
        let to = datetime!(2026-06-02 00:00:00 UTC);
        let busy = off_hours_intervals(&wk, from, to);
        assert_eq!(busy.len(), 2);
        assert_eq!(busy[0].start, from);
        assert_eq!(busy[0].end, datetime!(2026-06-01 09:00:00 UTC));
        assert_eq!(busy[1].start, datetime!(2026-06-01 17:00:00 UTC));
        assert_eq!(busy[1].end, to);
    }

    #[test]
    fn full_day_window_yields_no_busy() {
        let mut wk = empty_week();
        wk[1] = vec![(0, 1440)];
        let from = datetime!(2026-06-01 00:00:00 UTC);
        let to = datetime!(2026-06-02 00:00:00 UTC);
        assert!(off_hours_intervals(&wk, from, to).is_empty());
    }

    #[test]
    fn clamps_to_query_window() {
        // Query starts mid-morning; the pre-window busy gap is clamped to `from`.
        let mut wk = empty_week();
        wk[1] = vec![(9 * 60, 17 * 60)];
        let from = datetime!(2026-06-01 10:00:00 UTC);
        let to = datetime!(2026-06-01 12:00:00 UTC);
        // Whole query window is inside working hours → no busy.
        assert!(off_hours_intervals(&wk, from, to).is_empty());
    }
}

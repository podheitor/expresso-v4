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
    /// For each attendee with working hours configured, every gap *outside* their
    /// windows becomes a busy interval (so the scheduler avoids off-hours slots).
    /// Windows are stored as minutes-from-midnight in the user's IANA timezone
    /// (from `users.timezone`); we convert each local day's boundaries to UTC via
    /// jiff (DST-aware). Attendees with no working-hours rows are omitted.
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
            "SELECT lower(u.email) AS email, u.timezone, w.weekday, w.start_minute, w.end_minute \
               FROM user_working_hours w \
               JOIN users u ON u.id = w.user_id AND u.tenant_id = w.tenant_id \
              WHERE w.tenant_id = $1 AND lower(u.email) = ANY($2)",
        )
        .bind(tenant_id)
        .bind(&lowered)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        // Group windows per email per weekday, keeping the user's timezone.
        let mut by_email: BTreeMap<String, (String, WeekWindows)> = BTreeMap::new();
        for r in rows {
            let entry = by_email
                .entry(r.email)
                .or_insert_with(|| (r.timezone.clone(), Default::default()));
            let wd = r.weekday.clamp(0, 6) as usize;
            entry.1[wd].push((r.start_minute, r.end_minute));
        }

        for (email, (tz, by_day)) in by_email {
            let busy = off_hours_intervals(&tz, &by_day, from, to);
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
    timezone: String,
    weekday: i16,
    start_minute: i32,
    end_minute: i32,
}

/// Per-weekday working windows (Sunday=0..Saturday=6); each a list of
/// (start_minute, end_minute) in the user's local timezone.
type WeekWindows = [Vec<(i32, i32)>; 7];

/// Off-hours busy intervals for one attendee, with per-weekday working windows
/// expressed in the IANA `tz`. Walks each civil day in the user's timezone,
/// converts the local window boundaries to UTC instants (jiff resolves the
/// offset, including DST), and emits the gaps (before/between/after windows; a
/// weekday with no windows is fully busy) clamped to [from, to].
fn off_hours_intervals(
    tz: &str,
    by_day: &WeekWindows,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Vec<BusyInterval> {
    // Resolve the timezone; fall back to UTC for unknown names (best-effort).
    let zone = jiff::tz::TimeZone::get(tz).unwrap_or(jiff::tz::TimeZone::UTC);

    // The civil date range to scan, in the user's tz: from the local date of
    // `from` through the local date of `to` (inclusive of the last partial day).
    let start_local = utc_to_zoned_date(&zone, from);
    let end_local = utc_to_zoned_date(&zone, to);
    let mut out = Vec::new();
    let mut date = start_local;
    while date <= end_local {
        // jiff weekday: Sunday=0..Saturday=6 to match the stored convention.
        let wd = date.weekday().to_sunday_zero_offset() as usize;
        let mut windows = by_day[wd].clone();
        windows.sort_unstable();
        let mut cursor = 0i32; // local minutes from midnight
        for (ws, we) in windows {
            if ws > cursor {
                push_local(&mut out, &zone, date, cursor, ws, from, to);
            }
            cursor = cursor.max(we);
        }
        if cursor < 1440 {
            push_local(&mut out, &zone, date, cursor, 1440, from, to);
        }
        date = date.tomorrow().unwrap_or(date);
        // Guard against the tomorrow()-overflow fixpoint at the max date.
        if date == end_local && start_local == end_local {
            break;
        }
    }
    out
}

/// The civil date (in `zone`) of a UTC instant.
fn utc_to_zoned_date(zone: &jiff::tz::TimeZone, t: OffsetDateTime) -> jiff::civil::Date {
    let ts =
        jiff::Timestamp::from_second(t.unix_timestamp()).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    ts.to_zoned(zone.clone()).date()
}

/// Convert a local `date` + [start_min, end_min) window in `zone` to a UTC busy
/// interval, clamped to [from, to]. DST-aware via jiff's civil→zoned resolution.
fn push_local(
    out: &mut Vec<BusyInterval>,
    zone: &jiff::tz::TimeZone,
    date: jiff::civil::Date,
    start_min: i32,
    end_min: i32,
    from: OffsetDateTime,
    to: OffsetDateTime,
) {
    let Some(s_utc) = local_minutes_to_utc(zone, date, start_min) else {
        return;
    };
    let Some(e_utc) = local_minutes_to_utc(zone, date, end_min) else {
        return;
    };
    let s = s_utc.max(from);
    let e = e_utc.min(to);
    if e > s {
        out.push(BusyInterval { start: s, end: e });
    }
}

/// Local `date` + `minutes`-from-midnight in `zone` → UTC `OffsetDateTime`.
/// `minutes` may be 1440 (end-of-day) → rolls to 00:00 next day.
fn local_minutes_to_utc(
    zone: &jiff::tz::TimeZone,
    date: jiff::civil::Date,
    minutes: i32,
) -> Option<OffsetDateTime> {
    let (d, hm) = if minutes >= 1440 {
        (date.tomorrow().ok()?, 0)
    } else {
        (date, minutes)
    };
    let civil = d.at((hm / 60) as i8, (hm % 60) as i8, 0, 0);
    // Resolve civil→zoned (DST: pick the compatible instant). Then to UTC secs.
    let zoned = zone.to_zoned(civil).ok()?;
    OffsetDateTime::from_unix_timestamp(zoned.timestamp().as_second()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn empty_week() -> [Vec<(i32, i32)>; 7] {
        Default::default()
    }

    #[test]
    fn no_windows_means_whole_span_busy_utc() {
        let from = datetime!(2026-06-01 00:00:00 UTC);
        let to = datetime!(2026-06-02 00:00:00 UTC);
        let busy = off_hours_intervals("UTC", &empty_week(), from, to);
        assert_eq!(busy.len(), 1);
        assert_eq!(busy[0].start, from);
        assert_eq!(busy[0].end, to);
    }

    #[test]
    fn window_splits_day_utc() {
        // Monday 09:00-17:00 UTC working → busy [00:00,09:00) and [17:00,24:00).
        let mut wk = empty_week();
        wk[1] = vec![(9 * 60, 17 * 60)]; // Monday=1
        let from = datetime!(2026-06-01 00:00:00 UTC);
        let to = datetime!(2026-06-02 00:00:00 UTC);
        let busy = off_hours_intervals("UTC", &wk, from, to);
        assert_eq!(busy.len(), 2);
        assert_eq!(busy[0].start, from);
        assert_eq!(busy[0].end, datetime!(2026-06-01 09:00:00 UTC));
        assert_eq!(busy[1].start, datetime!(2026-06-01 17:00:00 UTC));
        assert_eq!(busy[1].end, to);
    }

    #[test]
    fn full_day_window_yields_no_busy_utc() {
        let mut wk = empty_week();
        wk[1] = vec![(0, 1440)];
        let from = datetime!(2026-06-01 00:00:00 UTC);
        let to = datetime!(2026-06-02 00:00:00 UTC);
        assert!(off_hours_intervals("UTC", &wk, from, to).is_empty());
    }

    #[test]
    fn timezone_shifts_window_to_utc() {
        // Monday 09:00-17:00 in America/Sao_Paulo (UTC-3, no DST in 2026) maps to
        // 12:00-20:00 UTC. So the morning busy gap should END at 12:00 UTC.
        let mut wk = empty_week();
        wk[1] = vec![(9 * 60, 17 * 60)];
        let from = datetime!(2026-06-01 00:00:00 UTC);
        let to = datetime!(2026-06-02 00:00:00 UTC);
        let busy = off_hours_intervals("America/Sao_Paulo", &wk, from, to);
        // First busy interval (the morning gap) ends at 12:00 UTC (= 09:00 -03).
        assert!(busy
            .iter()
            .any(|b| b.end == datetime!(2026-06-01 12:00:00 UTC)));
        // And a busy interval starts at 20:00 UTC (= 17:00 -03).
        assert!(busy
            .iter()
            .any(|b| b.start == datetime!(2026-06-01 20:00:00 UTC)));
    }

    #[test]
    fn unknown_tz_falls_back_to_utc() {
        let mut wk = empty_week();
        wk[1] = vec![(9 * 60, 17 * 60)];
        let from = datetime!(2026-06-01 00:00:00 UTC);
        let to = datetime!(2026-06-02 00:00:00 UTC);
        let busy = off_hours_intervals("Not/AZone", &wk, from, to);
        // Falls back to UTC: morning gap ends at 09:00 UTC.
        assert!(busy
            .iter()
            .any(|b| b.end == datetime!(2026-06-01 09:00:00 UTC)));
    }
}

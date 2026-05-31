//! Bookable calendar resources (rooms/equipment) + double-booking detection.
//!
//! A resource is identified by an email matching the `ATTENDEE;CUTYPE=ROOM|
//! RESOURCE` convention. The registry (`calendar_resources`) is tenant-scoped;
//! events record which resources they book in `calendar_event_resources`
//! (synced by `EventRepo`). A double-booking is two events that share a resource
//! email and whose occurrences overlap in time within the query window. RRULE is
//! expanded per the shared `expresso-rrule` crate, so a recurring booking
//! conflicts on every occurrence — not just its master instance.

use expresso_core::{begin_tenant_tx, DbPool};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{CalendarError, Result};

/// Maximum length accepted for a resource email/name (abuse guard).
const MAX_FIELD_BYTES: usize = 320;

/// A registered bookable resource.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Resource {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub email: String,
    pub name: String,
    pub kind: String,
    pub capacity: Option<i32>,
    pub is_active: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// New-resource payload (registry create).
#[derive(Debug, Clone, Deserialize)]
pub struct NewResource {
    pub email: String,
    pub name: String,
    pub kind: Option<String>,
    pub capacity: Option<i32>,
}

/// One double-booking: two events overlapping on the same resource. `a_dtstart`/
/// `a_dtend` are the master event times; `overlap_start`/`overlap_end` are the
/// first overlapping *occurrence* window (equal to the master times for
/// non-recurring events, but a later instance for recurring ones).
#[derive(Debug, Clone, Serialize)]
pub struct ResourceConflict {
    pub resource_email: String,
    pub a_event_id: Uuid,
    pub a_summary: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub a_dtstart: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub a_dtend: Option<OffsetDateTime>,
    pub b_event_id: Uuid,
    pub b_summary: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub b_dtstart: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub b_dtend: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub overlap_start: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub overlap_end: OffsetDateTime,
}

/// A candidate event pair sharing a resource, fetched before RRULE expansion.
/// Both rows are guaranteed to have `dtstart`; `dtend`/`rrule` may be absent.
#[derive(Debug, FromRow)]
struct ConflictCandidate {
    resource_email: String,
    a_event_id: Uuid,
    a_summary: Option<String>,
    a_dtstart: OffsetDateTime,
    a_dtend: Option<OffsetDateTime>,
    a_rrule: Option<String>,
    b_event_id: Uuid,
    b_summary: Option<String>,
    b_dtstart: OffsetDateTime,
    b_dtend: Option<OffsetDateTime>,
    b_rrule: Option<String>,
}

pub struct ResourceRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> ResourceRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Register a resource. 409 on duplicate email within the tenant.
    pub async fn create(&self, tenant: Uuid, n: NewResource) -> Result<Resource> {
        let email = normalize_email(&n.email)?;
        let name = n.name.trim();
        if name.is_empty() || name.len() > MAX_FIELD_BYTES {
            return Err(CalendarError::BadRequest("invalid name".into()));
        }
        let kind = match n.kind.as_deref() {
            None | Some("room") => "room",
            Some("resource") => "resource",
            Some(_) => {
                return Err(CalendarError::BadRequest(
                    "kind must be room|resource".into(),
                ))
            }
        };

        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, Resource>(
            r#"INSERT INTO calendar_resources (tenant_id, email, name, kind, capacity)
               VALUES ($1, $2, $3, $4, $5)
               RETURNING id, tenant_id, email, name, kind, capacity, is_active,
                         created_at, updated_at"#,
        )
        .bind(tenant)
        .bind(&email)
        .bind(name)
        .bind(kind)
        .bind(n.capacity)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                CalendarError::Conflict(format!("resource already registered: {email}"))
            }
            other => CalendarError::Database(other),
        })?;
        tx.commit().await?;
        Ok(row)
    }

    /// List the tenant's resources (active first, then by name).
    pub async fn list(&self, tenant: Uuid) -> Result<Vec<Resource>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows = sqlx::query_as::<_, Resource>(
            r#"SELECT id, tenant_id, email, name, kind, capacity, is_active,
                      created_at, updated_at
                 FROM calendar_resources
                WHERE tenant_id = $1
                ORDER BY is_active DESC, name"#,
        )
        .bind(tenant)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Fetch one resource by id. 404 if absent in tenant.
    pub async fn get(&self, tenant: Uuid, id: Uuid) -> Result<Resource> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, Resource>(
            r#"SELECT id, tenant_id, email, name, kind, capacity, is_active,
                      created_at, updated_at
                 FROM calendar_resources WHERE tenant_id = $1 AND id = $2"#,
        )
        .bind(tenant)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(CalendarError::ResourceNotFound(id))?;
        tx.commit().await?;
        Ok(row)
    }

    /// Delete a resource from the registry. Does NOT touch event bookings — an
    /// event keeps its CUTYPE=ROOM attendee; the room is simply unregistered.
    /// 404 when nothing was removed.
    pub async fn delete(&self, tenant: Uuid, id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let res = sqlx::query("DELETE FROM calendar_resources WHERE tenant_id = $1 AND id = $2")
            .bind(tenant)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if res.rows_affected() == 0 {
            return Err(CalendarError::ResourceNotFound(id));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Find double-bookings of `resource_email` within `[from, to)`: event pairs
    /// that both book the resource and have at least one pair of occurrences
    /// overlapping in time (half-open). Each event's RRULE is expanded, so a
    /// recurring booking conflicts on every instance, not just its master.
    /// Each pair is returned once (`a_event_id < b_event_id`) at its first
    /// overlapping occurrence.
    ///
    /// The SQL only pre-filters by shared resource + master instance touching the
    /// window; it deliberately does NOT apply a master-vs-master time overlap,
    /// because two recurring events can collide on a later occurrence while their
    /// masters do not. The occurrence-level overlap is decided in Rust below.
    pub async fn conflicts(
        &self,
        tenant: Uuid,
        resource_email: &str,
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> Result<Vec<ResourceConflict>> {
        let email = resource_email.trim().to_ascii_lowercase();
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let candidates = sqlx::query_as::<_, ConflictCandidate>(
            r#"SELECT ra.resource_email AS resource_email,
                      ea.id AS a_event_id, ea.summary AS a_summary,
                      ea.dtstart AS a_dtstart, ea.dtend AS a_dtend, ea.rrule AS a_rrule,
                      eb.id AS b_event_id, eb.summary AS b_summary,
                      eb.dtstart AS b_dtstart, eb.dtend AS b_dtend, eb.rrule AS b_rrule
                 FROM calendar_event_resources ra
                 JOIN calendar_event_resources rb
                   ON rb.tenant_id      = ra.tenant_id
                  AND rb.resource_email = ra.resource_email
                  AND rb.event_id       > ra.event_id
                 JOIN calendar_events ea ON ea.id = ra.event_id AND ea.tenant_id = ra.tenant_id
                 JOIN calendar_events eb ON eb.id = rb.event_id AND eb.tenant_id = rb.tenant_id
                WHERE ra.tenant_id = $1
                  AND ra.resource_email = $2
                  AND ea.dtstart IS NOT NULL
                  AND eb.dtstart IS NOT NULL
                  AND (ea.rrule IS NOT NULL OR (ea.dtend > $3 AND ea.dtstart < $4))
                  AND (eb.rrule IS NOT NULL OR (eb.dtend > $3 AND eb.dtstart < $4))
                ORDER BY ea.dtstart, eb.dtstart"#,
        )
        .bind(tenant)
        .bind(&email)
        .bind(from)
        .bind(to)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(candidates
            .into_iter()
            .filter_map(|c| first_overlap(&c, from, to))
            .collect())
    }
}

/// Expand both events' occurrences within `[from, to)` and return the earliest
/// pair that overlaps (half-open), as a `ResourceConflict`. `None` when no
/// occurrence pair collides — the pair is not a double-booking in this window.
fn first_overlap(
    c: &ConflictCandidate,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Option<ResourceConflict> {
    let occ_a = occurrences(c.a_dtstart, c.a_dtend, c.a_rrule.as_deref(), from, to);
    let occ_b = occurrences(c.b_dtstart, c.b_dtend, c.b_rrule.as_deref(), from, to);
    let (overlap_start, overlap_end) = occ_a.iter().find_map(|&(as_, ae)| {
        occ_b
            .iter()
            .find(|&&(bs, be)| as_ < be && bs < ae)
            .map(|&(bs, be)| (as_.max(bs), ae.min(be)))
    })?;
    Some(ResourceConflict {
        resource_email: c.resource_email.clone(),
        a_event_id: c.a_event_id,
        a_summary: c.a_summary.clone(),
        a_dtstart: Some(c.a_dtstart),
        a_dtend: c.a_dtend,
        b_event_id: c.b_event_id,
        b_summary: c.b_summary.clone(),
        b_dtstart: Some(c.b_dtstart),
        b_dtend: c.b_dtend,
        overlap_start,
        overlap_end,
    })
}

/// Concrete `(start, end)` occurrence windows of an event within `[from, to)`,
/// expanding RRULE when present and falling back to the single master instance
/// otherwise (mirrors `FreeBusyRepo::lookup`).
fn occurrences(
    dtstart: OffsetDateTime,
    dtend: Option<OffsetDateTime>,
    rrule: Option<&str>,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Vec<(OffsetDateTime, OffsetDateTime)> {
    let duration = dtend.unwrap_or(dtstart) - dtstart;
    match rrule.and_then(super::rrule::Rrule::parse) {
        Some(rule) => rule.expand(dtstart, duration, from, to),
        None => super::rrule::single_instance(dtstart, dtend, from, to)
            .into_iter()
            .collect(),
    }
}

/// Normalize + validate a resource email: trimmed, lowercased, non-empty, has an
/// `@`, within the byte cap.
fn normalize_email(raw: &str) -> Result<String> {
    let email = raw.trim().to_ascii_lowercase();
    if email.is_empty() || email.len() > MAX_FIELD_BYTES || !email.contains('@') {
        return Err(CalendarError::BadRequest("invalid email".into()));
    }
    Ok(email)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_email_lowercases_and_trims() {
        assert_eq!(normalize_email("  Room-A@X.org ").unwrap(), "room-a@x.org");
    }

    #[test]
    fn normalize_email_rejects_missing_at() {
        assert!(normalize_email("not-an-email").is_err());
    }

    #[test]
    fn normalize_email_rejects_empty() {
        assert!(normalize_email("   ").is_err());
    }

    #[test]
    fn new_resource_deserializes_minimal() {
        let n: NewResource =
            serde_json::from_str(r#"{"email":"r@x.org","name":"Sala 4"}"#).unwrap();
        assert_eq!(n.email, "r@x.org");
        assert_eq!(n.name, "Sala 4");
        assert!(n.kind.is_none());
        assert!(n.capacity.is_none());
    }

    #[test]
    fn new_resource_deserializes_full() {
        let n: NewResource = serde_json::from_str(
            r#"{"email":"r@x.org","name":"Sala","kind":"room","capacity":12}"#,
        )
        .unwrap();
        assert_eq!(n.kind.as_deref(), Some("room"));
        assert_eq!(n.capacity, Some(12));
    }

    use time::macros::datetime;

    fn candidate(
        a_start: OffsetDateTime,
        a_end: OffsetDateTime,
        a_rrule: Option<&str>,
        b_start: OffsetDateTime,
        b_end: OffsetDateTime,
        b_rrule: Option<&str>,
    ) -> ConflictCandidate {
        ConflictCandidate {
            resource_email: "room-a@x.org".into(),
            a_event_id: Uuid::nil(),
            a_summary: Some("A".into()),
            a_dtstart: a_start,
            a_dtend: Some(a_end),
            a_rrule: a_rrule.map(str::to_owned),
            b_event_id: Uuid::from_u128(1),
            b_summary: Some("B".into()),
            b_dtstart: b_start,
            b_dtend: Some(b_end),
            b_rrule: b_rrule.map(str::to_owned),
        }
    }

    #[test]
    fn non_recurring_same_slot_conflicts() {
        let c = candidate(
            datetime!(2026-06-01 09:00 UTC),
            datetime!(2026-06-01 10:00 UTC),
            None,
            datetime!(2026-06-01 09:30 UTC),
            datetime!(2026-06-01 10:30 UTC),
            None,
        );
        let hit = first_overlap(
            &c,
            datetime!(2026-06-01 00:00 UTC),
            datetime!(2026-06-02 00:00 UTC),
        )
        .expect("overlap");
        assert_eq!(hit.overlap_start, datetime!(2026-06-01 09:30 UTC));
        assert_eq!(hit.overlap_end, datetime!(2026-06-01 10:00 UTC));
    }

    #[test]
    fn non_recurring_disjoint_does_not_conflict() {
        let c = candidate(
            datetime!(2026-06-01 09:00 UTC),
            datetime!(2026-06-01 10:00 UTC),
            None,
            datetime!(2026-06-01 11:00 UTC),
            datetime!(2026-06-01 12:00 UTC),
            None,
        );
        assert!(first_overlap(
            &c,
            datetime!(2026-06-01 00:00 UTC),
            datetime!(2026-06-02 00:00 UTC)
        )
        .is_none());
    }

    /// The regression this sprint fixes: a weekly recurring booking whose master
    /// instance is on a different day than the one-off, but a LATER occurrence
    /// lands on the same slot. The old stored-dtstart-only check missed this.
    #[test]
    fn recurring_occurrence_conflicts_even_when_masters_dont() {
        // A: weekly Mondays 09:00–10:00 starting 2026-06-01 (a Monday).
        // B: one-off on 2026-06-15 (the 3rd Monday) 09:30–10:30.
        let c = candidate(
            datetime!(2026-06-01 09:00 UTC),
            datetime!(2026-06-01 10:00 UTC),
            Some("FREQ=WEEKLY;BYDAY=MO"),
            datetime!(2026-06-15 09:30 UTC),
            datetime!(2026-06-15 10:30 UTC),
            None,
        );
        let hit = first_overlap(
            &c,
            datetime!(2026-06-01 00:00 UTC),
            datetime!(2026-07-01 00:00 UTC),
        )
        .expect("recurring occurrence should conflict");
        // Master A (06-01) does NOT overlap B (06-15); the 06-15 occurrence does.
        assert_eq!(hit.overlap_start, datetime!(2026-06-15 09:30 UTC));
        assert_eq!(hit.overlap_end, datetime!(2026-06-15 10:00 UTC));
        // Master times are still reported alongside the occurrence window.
        assert_eq!(hit.a_dtstart, Some(datetime!(2026-06-01 09:00 UTC)));
    }

    #[test]
    fn recurring_on_off_weeks_does_not_conflict() {
        // A: weekly Mondays; B: one-off on a Tuesday → never the same day.
        let c = candidate(
            datetime!(2026-06-01 09:00 UTC),
            datetime!(2026-06-01 10:00 UTC),
            Some("FREQ=WEEKLY;BYDAY=MO"),
            datetime!(2026-06-09 09:00 UTC),
            datetime!(2026-06-09 10:00 UTC),
            None,
        );
        assert!(first_overlap(
            &c,
            datetime!(2026-06-01 00:00 UTC),
            datetime!(2026-07-01 00:00 UTC)
        )
        .is_none());
    }

    #[test]
    fn occurrences_falls_back_to_master_when_no_rrule() {
        let occ = occurrences(
            datetime!(2026-06-01 09:00 UTC),
            Some(datetime!(2026-06-01 10:00 UTC)),
            None,
            datetime!(2026-06-01 00:00 UTC),
            datetime!(2026-06-02 00:00 UTC),
        );
        assert_eq!(
            occ,
            vec![(
                datetime!(2026-06-01 09:00 UTC),
                datetime!(2026-06-01 10:00 UTC)
            )]
        );
    }
}

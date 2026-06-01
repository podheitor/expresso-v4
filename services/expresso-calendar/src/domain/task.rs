//! Calendar tasks (RFC 5545 VTODO) — domain model + repository.
//!
//! A task belongs to a calendar and inherits its tenant + ACL. Storage is
//! `calendar_tasks` (RLS by `app.tenant_id`). The repo mirrors `EventRepo`:
//! `begin_tenant_tx` for tenant scoping, `RETURNING *` for round-trips. CalDAV
//! VTODO serialization is a later slice; this layer is REST-first.

use expresso_core::{begin_tenant_tx, DbPool};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{CalendarError, Result};

/// Max length accepted for a task summary (abuse guard); description is capped
/// larger since notes can be long.
const MAX_SUMMARY_BYTES: usize = 1024;
const MAX_DESCRIPTION_BYTES: usize = 64 * 1024;

/// The four VTODO STATUS values we accept (RFC 5545 §3.8.1.11).
const VALID_STATUS: [&str; 4] = ["NEEDS-ACTION", "IN-PROCESS", "COMPLETED", "CANCELLED"];

/// A stored task.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Task {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub calendar_id: Uuid,
    pub uid: String,
    pub summary: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: i16,
    pub percent_complete: i16,
    #[serde(with = "time::serde::rfc3339::option")]
    pub dtstart: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub due: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub completed_at: Option<OffsetDateTime>,
    /// RFC 5545 recurrence rule (e.g. "FREQ=WEEKLY;BYDAY=MO"), or None for a
    /// one-off task. Expanded virtually via [`expand_instances`](Self::expand_instances).
    pub rrule: Option<String>,
    /// Verbatim VCALENDAR body when created over CalDAV; None for REST-created
    /// tasks (CalDAV GET serializes those from the columns on the fly).
    #[serde(skip)]
    pub ical_raw: Option<String>,
    /// SHA-256 of `ical_raw` for CalDAV If-Match/sync; None when REST-created.
    #[serde(skip)]
    pub etag: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl Task {
    /// The task's CalDAV ETag: the stored one when present (CalDAV-created),
    /// else computed from the serialized body so REST-created tasks still get a
    /// stable validator.
    pub fn caldav_etag(&self) -> String {
        match &self.etag {
            Some(e) => e.clone(),
            None => crate::domain::ical::compute_etag(&self.to_ical()),
        }
    }

    /// The task's VCALENDAR/VTODO body: the stored verbatim source when present,
    /// else serialized from the structured fields.
    pub fn to_ical(&self) -> String {
        if let Some(raw) = &self.ical_raw {
            return raw.clone();
        }
        crate::domain::ical::serialize_vtodo(&crate::domain::ical::Vtodo {
            uid: &self.uid,
            summary: &self.summary,
            description: self.description.as_deref(),
            status: &self.status,
            priority: self.priority,
            percent_complete: self.percent_complete,
            dtstart: self.dtstart,
            due: self.due,
            completed: self.completed_at,
            rrule: self.rrule.as_deref(),
        })
    }

    /// Expand the task's recurrence into concrete due-dates within `[from, to)`.
    /// The anchor is `dtstart` when present, else `due`; with no anchor the task
    /// has no timeline and expansion is empty. A one-off (no/invalid rrule)
    /// yields its single anchor when it falls in the window. Returns each
    /// occurrence's anchor instant (VTODO has no duration, unlike VEVENT).
    pub fn expand_instances(
        &self,
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> Vec<OffsetDateTime> {
        let Some(anchor) = self.dtstart.or(self.due) else {
            return Vec::new();
        };
        match self.rrule.as_deref().filter(|r| !r.trim().is_empty()) {
            Some(raw) => match crate::domain::rrule::Rrule::parse(raw) {
                Some(rule) => rule
                    .expand(anchor, time::Duration::ZERO, from, to)
                    .into_iter()
                    .map(|(s, _)| s)
                    .collect(),
                None => single_anchor(anchor, from, to),
            },
            None => single_anchor(anchor, from, to),
        }
    }
}

/// A non-recurring task's single occurrence: the anchor if it lies in `[from, to)`.
fn single_anchor(
    anchor: OffsetDateTime,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Vec<OffsetDateTime> {
    if anchor >= from && anchor < to {
        vec![anchor]
    } else {
        Vec::new()
    }
}

/// Create payload. `uid` is optional — generated when absent.
#[derive(Debug, Clone, Deserialize)]
pub struct NewTask {
    pub uid: Option<String>,
    pub summary: String,
    pub description: Option<String>,
    pub status: Option<String>,
    pub priority: Option<i16>,
    pub percent_complete: Option<i16>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub dtstart: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub due: Option<OffsetDateTime>,
    /// Optional RFC 5545 recurrence rule. Stored verbatim; validated lazily at
    /// expansion (an unparseable rule degrades to a one-off).
    pub rrule: Option<String>,
}

/// Partial update — only present fields change. `status` flipping to/from
/// COMPLETED also drives `completed_at` and `percent_complete` (see `update`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateTask {
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    pub status: Option<String>,
    pub priority: Option<i16>,
    pub percent_complete: Option<i16>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub dtstart: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub due: Option<OffsetDateTime>,
    /// Doubly-optional: `null` clears the rule (make one-off), a string sets it,
    /// absent leaves it.
    #[serde(default)]
    pub rrule: Option<Option<String>>,
}

pub struct TaskRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> TaskRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Insert a task into `calendar_id`. 409 on duplicate `(calendar_id, uid)`.
    pub async fn create(&self, tenant: Uuid, calendar_id: Uuid, n: NewTask) -> Result<Task> {
        let summary = validate_summary(&n.summary)?;
        let description = validate_description(n.description.as_deref())?;
        let status = validate_status(n.status.as_deref())?;
        let priority = validate_priority(n.priority)?;
        let percent = validate_percent(n.percent_complete)?;
        let uid = n
            .uid
            .map(|u| u.trim().to_owned())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| format!("task-{}", Uuid::new_v4()));
        let completed_at = completed_now(status, percent);

        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rrule = clean_rrule(n.rrule.as_deref());
        let row = sqlx::query_as::<_, Task>(
            r#"INSERT INTO calendar_tasks
                   (tenant_id, calendar_id, uid, summary, description, status,
                    priority, percent_complete, dtstart, due, completed_at, rrule)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
               RETURNING *"#,
        )
        .bind(tenant)
        .bind(calendar_id)
        .bind(&uid)
        .bind(summary)
        .bind(description)
        .bind(status)
        .bind(priority)
        .bind(percent)
        .bind(n.dtstart)
        .bind(n.due)
        .bind(completed_at)
        .bind(rrule)
        .fetch_one(&mut *tx)
        .await
        .map_err(dup_to_conflict)?;
        tx.commit().await?;
        Ok(row)
    }

    /// List a calendar's tasks, open ones first (status order), then by due.
    pub async fn list(&self, tenant: Uuid, calendar_id: Uuid) -> Result<Vec<Task>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows = sqlx::query_as::<_, Task>(
            r#"SELECT * FROM calendar_tasks
                WHERE tenant_id = $1 AND calendar_id = $2
                ORDER BY (status IN ('COMPLETED','CANCELLED')), due NULLS LAST, created_at"#,
        )
        .bind(tenant)
        .bind(calendar_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Fetch one task by id within a calendar. 404 if absent.
    pub async fn get(&self, tenant: Uuid, calendar_id: Uuid, id: Uuid) -> Result<Task> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, Task>(
            r#"SELECT * FROM calendar_tasks
                WHERE tenant_id = $1 AND calendar_id = $2 AND id = $3"#,
        )
        .bind(tenant)
        .bind(calendar_id)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(CalendarError::TaskNotFound(id))?;
        tx.commit().await?;
        Ok(row)
    }

    /// Apply a partial update. Validates each present field. When `status`
    /// becomes COMPLETED, `completed_at`/`percent_complete` are set; leaving
    /// COMPLETED clears `completed_at`. 404 if the task is absent.
    pub async fn update(
        &self,
        tenant: Uuid,
        calendar_id: Uuid,
        id: Uuid,
        u: UpdateTask,
    ) -> Result<Task> {
        let current = self.get(tenant, calendar_id, id).await?;
        let summary = match &u.summary {
            Some(s) => validate_summary(s)?.to_owned(),
            None => current.summary,
        };
        let description = match u.description {
            Some(d) => validate_description(d.as_deref())?.map(str::to_owned),
            None => current.description,
        };
        let status = match u.status.as_deref() {
            Some(s) => validate_status(Some(s))?,
            None => leak_status(&current.status),
        };
        let priority = validate_priority(u.priority.or(Some(current.priority)))?;
        let percent = validate_percent(u.percent_complete.or(Some(current.percent_complete)))?;
        let percent = if status == "COMPLETED" { 100 } else { percent };
        let completed_at = completed_now(status, percent);
        let dtstart = u.dtstart.or(current.dtstart);
        let due = u.due.or(current.due);
        let rrule = match u.rrule {
            Some(r) => clean_rrule(r.as_deref()),
            None => current.rrule,
        };

        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, Task>(
            r#"UPDATE calendar_tasks
                  SET summary = $4, description = $5, status = $6, priority = $7,
                      percent_complete = $8, dtstart = $9, due = $10, completed_at = $11,
                      rrule = $12
                WHERE tenant_id = $1 AND calendar_id = $2 AND id = $3
                RETURNING *"#,
        )
        .bind(tenant)
        .bind(calendar_id)
        .bind(id)
        .bind(summary)
        .bind(description)
        .bind(status)
        .bind(priority)
        .bind(percent)
        .bind(dtstart)
        .bind(due)
        .bind(completed_at)
        .bind(rrule)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// CalDAV upsert from a raw VCALENDAR/VTODO body. Parses the VTODO for the
    /// structured columns, stores the body verbatim + its etag, keyed by
    /// `(calendar_id, uid)`. Mirrors `EventRepo::replace_by_uid`.
    pub async fn replace_by_uid(&self, tenant: Uuid, calendar_id: Uuid, raw: &str) -> Result<Task> {
        let parsed = crate::domain::ical::parse_vtodo(raw)?;
        let etag = crate::domain::ical::compute_etag(raw);
        let summary = validate_summary(parsed.summary.as_deref().unwrap_or(""))?;
        let status = validate_status(parsed.status.as_deref())?;
        let completed_at = parsed
            .completed
            .or_else(|| completed_now(status, parsed.percent_complete));

        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rrule = clean_rrule(parsed.rrule.as_deref());
        let row = sqlx::query_as::<_, Task>(
            r#"INSERT INTO calendar_tasks
                   (tenant_id, calendar_id, uid, summary, description, status,
                    priority, percent_complete, dtstart, due, completed_at,
                    rrule, ical_raw, etag)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
               ON CONFLICT (calendar_id, uid) DO UPDATE SET
                   summary = EXCLUDED.summary, description = EXCLUDED.description,
                   status = EXCLUDED.status, priority = EXCLUDED.priority,
                   percent_complete = EXCLUDED.percent_complete,
                   dtstart = EXCLUDED.dtstart, due = EXCLUDED.due,
                   completed_at = EXCLUDED.completed_at, rrule = EXCLUDED.rrule,
                   ical_raw = EXCLUDED.ical_raw, etag = EXCLUDED.etag
               RETURNING *"#,
        )
        .bind(tenant)
        .bind(calendar_id)
        .bind(&parsed.uid)
        .bind(summary)
        .bind(parsed.description.as_deref())
        .bind(status)
        .bind(parsed.priority)
        .bind(parsed.percent_complete)
        .bind(parsed.dtstart)
        .bind(parsed.due)
        .bind(completed_at)
        .bind(rrule)
        .bind(raw)
        .bind(&etag)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Fetch one task by its iCalendar UID (CalDAV addressing). 404 if absent.
    pub async fn get_by_uid(&self, tenant: Uuid, calendar_id: Uuid, uid: &str) -> Result<Task> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, Task>(
            r#"SELECT * FROM calendar_tasks
                WHERE tenant_id = $1 AND calendar_id = $2 AND uid = $3"#,
        )
        .bind(tenant)
        .bind(calendar_id)
        .bind(uid)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| CalendarError::CalendarNotFound(uid.to_owned()))?;
        tx.commit().await?;
        Ok(row)
    }

    /// Fetch tasks by a set of UIDs (CalDAV calendar-multiget). Missing UIDs are
    /// simply absent from the result.
    pub async fn list_by_uids(
        &self,
        tenant: Uuid,
        calendar_id: Uuid,
        uids: &[String],
    ) -> Result<Vec<Task>> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows = sqlx::query_as::<_, Task>(
            r#"SELECT * FROM calendar_tasks
                WHERE tenant_id = $1 AND calendar_id = $2 AND uid = ANY($3)
                ORDER BY created_at"#,
        )
        .bind(tenant)
        .bind(calendar_id)
        .bind(uids)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Delete a task by its iCalendar UID (CalDAV DELETE). 404 if absent.
    pub async fn delete_by_uid(&self, tenant: Uuid, calendar_id: Uuid, uid: &str) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let res = sqlx::query(
            "DELETE FROM calendar_tasks WHERE tenant_id = $1 AND calendar_id = $2 AND uid = $3",
        )
        .bind(tenant)
        .bind(calendar_id)
        .bind(uid)
        .execute(&mut *tx)
        .await?;
        if res.rows_affected() == 0 {
            return Err(CalendarError::CalendarNotFound(uid.to_owned()));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Delete a task by id. 404 when nothing was removed.
    pub async fn delete(&self, tenant: Uuid, calendar_id: Uuid, id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let res = sqlx::query(
            "DELETE FROM calendar_tasks WHERE tenant_id = $1 AND calendar_id = $2 AND id = $3",
        )
        .bind(tenant)
        .bind(calendar_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if res.rows_affected() == 0 {
            return Err(CalendarError::TaskNotFound(id));
        }
        tx.commit().await?;
        Ok(())
    }
}

/// `completed_at = now()` exactly when the task is COMPLETED (or 100%); else
/// `None` so leaving the completed state clears the stamp.
fn completed_now(status: &str, percent: i16) -> Option<OffsetDateTime> {
    if status == "COMPLETED" || percent >= 100 {
        Some(OffsetDateTime::now_utc())
    } else {
        None
    }
}

/// Normalize an optional rrule: trim and treat blank as None, so an empty
/// string never persists as a "recurring" marker.
fn clean_rrule(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_owned)
}

/// Map a unique-violation on `(calendar_id, uid)` to a 409.
fn dup_to_conflict(e: sqlx::Error) -> CalendarError {
    match e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            CalendarError::Conflict("task uid already exists in calendar".into())
        }
        other => CalendarError::Database(other),
    }
}

/// Return a `&'static str` for one of the known statuses, so binds avoid an
/// allocation. The input is already validated to be one of `VALID_STATUS`.
fn leak_status(status: &str) -> &'static str {
    VALID_STATUS
        .iter()
        .copied()
        .find(|s| s.eq_ignore_ascii_case(status))
        .unwrap_or("NEEDS-ACTION")
}

fn validate_summary(raw: &str) -> Result<&str> {
    let s = raw.trim();
    if s.is_empty() || s.len() > MAX_SUMMARY_BYTES {
        return Err(CalendarError::BadRequest("invalid summary".into()));
    }
    Ok(s)
}

fn validate_description(raw: Option<&str>) -> Result<Option<&str>> {
    match raw {
        Some(d) if d.len() > MAX_DESCRIPTION_BYTES => {
            Err(CalendarError::BadRequest("description too long".into()))
        }
        other => Ok(other),
    }
}

fn validate_status(raw: Option<&str>) -> Result<&'static str> {
    match raw {
        None => Ok("NEEDS-ACTION"),
        Some(s) => VALID_STATUS
            .iter()
            .copied()
            .find(|v| v.eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| CalendarError::BadRequest("invalid status".into())),
    }
}

fn validate_priority(raw: Option<i16>) -> Result<i16> {
    match raw.unwrap_or(0) {
        p @ 0..=9 => Ok(p),
        _ => Err(CalendarError::BadRequest("priority must be 0..9".into())),
    }
}

fn validate_percent(raw: Option<i16>) -> Result<i16> {
    match raw.unwrap_or(0) {
        p @ 0..=100 => Ok(p),
        _ => Err(CalendarError::BadRequest(
            "percent_complete must be 0..100".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_defaults_and_normalizes_case() {
        assert_eq!(validate_status(None).unwrap(), "NEEDS-ACTION");
        assert_eq!(validate_status(Some("completed")).unwrap(), "COMPLETED");
        assert_eq!(validate_status(Some(" in-process ")).unwrap(), "IN-PROCESS");
    }

    #[test]
    fn status_rejects_unknown() {
        assert!(validate_status(Some("DONE")).is_err());
    }

    #[test]
    fn priority_and_percent_bounds() {
        assert!(validate_priority(Some(10)).is_err());
        assert_eq!(validate_priority(Some(5)).unwrap(), 5);
        assert!(validate_percent(Some(101)).is_err());
        assert_eq!(validate_percent(None).unwrap(), 0);
    }

    #[test]
    fn summary_must_be_nonempty() {
        assert!(validate_summary("   ").is_err());
        assert_eq!(validate_summary("  Buy milk ").unwrap(), "Buy milk");
    }

    #[test]
    fn completed_stamp_only_when_done() {
        assert!(completed_now("COMPLETED", 0).is_some());
        assert!(completed_now("NEEDS-ACTION", 100).is_some());
        assert!(completed_now("IN-PROCESS", 50).is_none());
    }

    #[test]
    fn leak_status_roundtrips_known() {
        assert_eq!(leak_status("completed"), "COMPLETED");
        assert_eq!(leak_status("bogus"), "NEEDS-ACTION");
    }

    #[test]
    fn clean_rrule_blanks_to_none() {
        assert_eq!(clean_rrule(None), None);
        assert_eq!(clean_rrule(Some("   ")), None);
        assert_eq!(
            clean_rrule(Some(" FREQ=DAILY ")).as_deref(),
            Some("FREQ=DAILY")
        );
    }

    fn task_with(rrule: Option<&str>, dtstart: Option<OffsetDateTime>) -> Task {
        Task {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            calendar_id: Uuid::nil(),
            uid: "u".into(),
            summary: "s".into(),
            description: None,
            status: "NEEDS-ACTION".into(),
            priority: 0,
            percent_complete: 0,
            dtstart,
            due: None,
            completed_at: None,
            rrule: rrule.map(str::to_owned),
            ical_raw: None,
            etag: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn expand_one_off_yields_anchor_in_window() {
        let anchor = time::macros::datetime!(2026-06-01 09:00 UTC);
        let t = task_with(None, Some(anchor));
        let from = time::macros::datetime!(2026-06-01 00:00 UTC);
        let to = time::macros::datetime!(2026-06-02 00:00 UTC);
        assert_eq!(t.expand_instances(from, to), vec![anchor]);
        // Outside the window → empty.
        let later = time::macros::datetime!(2026-07-01 00:00 UTC);
        let to2 = time::macros::datetime!(2026-07-02 00:00 UTC);
        assert!(t.expand_instances(later, to2).is_empty());
    }

    #[test]
    fn expand_without_anchor_is_empty() {
        let t = task_with(Some("FREQ=DAILY"), None);
        let from = time::macros::datetime!(2026-06-01 00:00 UTC);
        let to = time::macros::datetime!(2026-06-10 00:00 UTC);
        assert!(t.expand_instances(from, to).is_empty());
    }

    #[test]
    fn expand_daily_recurrence_counts_days() {
        let anchor = time::macros::datetime!(2026-06-01 09:00 UTC);
        let t = task_with(Some("FREQ=DAILY"), Some(anchor));
        let from = time::macros::datetime!(2026-06-01 00:00 UTC);
        let to = time::macros::datetime!(2026-06-04 00:00 UTC); // 3-day window
        assert_eq!(t.expand_instances(from, to).len(), 3);
    }
}

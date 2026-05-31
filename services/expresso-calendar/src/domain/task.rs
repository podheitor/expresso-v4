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
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
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
        let row = sqlx::query_as::<_, Task>(
            r#"INSERT INTO calendar_tasks
                   (tenant_id, calendar_id, uid, summary, description, status,
                    priority, percent_complete, dtstart, due, completed_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
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

        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, Task>(
            r#"UPDATE calendar_tasks
                  SET summary = $4, description = $5, status = $6, priority = $7,
                      percent_complete = $8, dtstart = $9, due = $10, completed_at = $11
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
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
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
}

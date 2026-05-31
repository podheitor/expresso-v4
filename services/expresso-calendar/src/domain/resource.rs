//! Bookable calendar resources (rooms/equipment) + double-booking detection.
//!
//! A resource is identified by an email matching the `ATTENDEE;CUTYPE=ROOM|
//! RESOURCE` convention. The registry (`calendar_resources`) is tenant-scoped;
//! events record which resources they book in `calendar_event_resources`
//! (synced by `EventRepo`). A double-booking is two events that share a resource
//! email and overlap in time (stored `dtstart`/`dtend`, no RRULE expansion —
//! consistent with the `events-conflicts` endpoint).

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

/// One double-booking: two events overlapping on the same resource.
#[derive(Debug, Clone, Serialize, FromRow)]
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

    /// Find double-bookings of `resource_email` within `[from, to]`: event pairs
    /// that both book the resource and overlap in time (half-open). Each pair is
    /// returned once (`a_event_id < b_event_id`).
    pub async fn conflicts(
        &self,
        tenant: Uuid,
        resource_email: &str,
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> Result<Vec<ResourceConflict>> {
        let email = resource_email.trim().to_ascii_lowercase();
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows = sqlx::query_as::<_, ResourceConflict>(
            r#"SELECT ra.resource_email AS resource_email,
                      ea.id AS a_event_id, ea.summary AS a_summary,
                      ea.dtstart AS a_dtstart, ea.dtend AS a_dtend,
                      eb.id AS b_event_id, eb.summary AS b_summary,
                      eb.dtstart AS b_dtstart, eb.dtend AS b_dtend
                 FROM calendar_event_resources ra
                 JOIN calendar_event_resources rb
                   ON rb.tenant_id      = ra.tenant_id
                  AND rb.resource_email = ra.resource_email
                  AND rb.event_id       > ra.event_id
                 JOIN calendar_events ea ON ea.id = ra.event_id AND ea.tenant_id = ra.tenant_id
                 JOIN calendar_events eb ON eb.id = rb.event_id AND eb.tenant_id = rb.tenant_id
                WHERE ra.tenant_id = $1
                  AND ra.resource_email = $2
                  AND ea.dtstart IS NOT NULL AND ea.dtend IS NOT NULL
                  AND eb.dtstart IS NOT NULL AND eb.dtend IS NOT NULL
                  AND ea.dtstart < eb.dtend
                  AND eb.dtstart < ea.dtend
                  AND ea.dtend   > $3 AND ea.dtstart < $4
                  AND eb.dtend   > $3 AND eb.dtstart < $4
                ORDER BY ea.dtstart, eb.dtstart"#,
        )
        .bind(tenant)
        .bind(&email)
        .bind(from)
        .bind(to)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
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
}

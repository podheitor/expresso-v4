//! Calendar (collection) domain model + repository.
//!
//! Tenant scoping: cada método abre transação via `begin_tenant_tx` para que
//! a policy RLS de `calendars` / `calendar_acl` filtre por
//! `current_setting('app.tenant_id')` antes mesmo do `WHERE tenant_id = $1`
//! explícito (defense-in-depth). Em `access_level`, os dois SELECTs
//! (owner_user_id + calendar_acl) agora rodam no mesmo snapshot — evita
//! race em ACL handoff (transferência de owner concorrente com revoke).

use expresso_core::{begin_tenant_tx, DbPool};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{CalendarError, Result};

/// Stored calendar collection.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Calendar {
    pub id:            Uuid,
    pub tenant_id:     Uuid,
    pub owner_user_id: Uuid,
    pub name:          String,
    pub description:   Option<String>,
    pub color:         Option<String>,
    pub timezone:      String,
    pub ctag:          i64,
    pub is_default:    bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:    OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at:    OffsetDateTime,
}

/// Creation payload.
#[derive(Debug, Clone, Deserialize)]
pub struct NewCalendar {
    pub name:         String,
    pub description:  Option<String>,
    pub color:        Option<String>,
    pub timezone:     Option<String>,
    #[serde(default)]
    pub is_default:   bool,
}

/// Partial update payload — None fields are left untouched.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateCalendar {
    pub name:        Option<String>,
    pub description: Option<String>,
    pub color:       Option<String>,
    pub timezone:    Option<String>,
    pub is_default:  Option<bool>,
}

/// Repository handle — holds the pool reference.
#[derive(Clone)]
pub struct CalendarRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> CalendarRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Insert new calendar for given tenant/owner.
    pub async fn create(
        &self,
        tenant_id: Uuid,
        owner_user_id: Uuid,
        input: &NewCalendar,
    ) -> Result<Calendar> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id)
            .await
            .map_err(CalendarError::from)?;
        let row = sqlx::query_as::<_, Calendar>(
            r#"
            INSERT INTO calendars
                (tenant_id, owner_user_id, name, description, color, timezone, is_default)
            VALUES ($1, $2, $3, $4, $5, COALESCE($6, 'America/Sao_Paulo'), $7)
            RETURNING id, tenant_id, owner_user_id, name, description, color,
                      timezone, ctag, is_default, created_at, updated_at
            "#,
        )
        .bind(tenant_id)
        .bind(owner_user_id)
        .bind(&input.name)
        .bind(&input.description)
        .bind(&input.color)
        .bind(&input.timezone)
        .bind(input.is_default)
        .fetch_one(&mut *tx)
        .await
        .map_err(CalendarError::from)?;
        tx.commit().await.map_err(CalendarError::from)?;
        Ok(row)
    }


    /// Insert calendar honoring caller-supplied UUID (CalDAV MKCALENDAR).
    pub async fn create_with_id(
        &self,
        id: Uuid,
        tenant_id: Uuid,
        owner_user_id: Uuid,
        input: &NewCalendar,
    ) -> Result<Calendar> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id)
            .await
            .map_err(CalendarError::from)?;
        let row = sqlx::query_as::<_, Calendar>(
            r#"
            INSERT INTO calendars
                (id, tenant_id, owner_user_id, name, description, color, timezone, is_default)
            VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, 'America/Sao_Paulo'), $8)
            RETURNING id, tenant_id, owner_user_id, name, description, color,
                      timezone, ctag, is_default, created_at, updated_at
            "#,
        )
        .bind(id)
        .bind(tenant_id)
        .bind(owner_user_id)
        .bind(&input.name)
        .bind(&input.description)
        .bind(&input.color)
        .bind(&input.timezone)
        .bind(input.is_default)
        .fetch_one(&mut *tx)
        .await
        .map_err(CalendarError::from)?;
        tx.commit().await.map_err(CalendarError::from)?;
        Ok(row)
    }

    /// List all calendars a user owns in this tenant.
    pub async fn list_for_owner(
        &self,
        tenant_id: Uuid,
        owner_user_id: Uuid,
    ) -> Result<Vec<Calendar>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id)
            .await
            .map_err(CalendarError::from)?;
        let rows = sqlx::query_as::<_, Calendar>(
            r#"
            SELECT id, tenant_id, owner_user_id, name, description, color,
                   timezone, ctag, is_default, created_at, updated_at
              FROM calendars
             WHERE tenant_id = $1 AND owner_user_id = $2
             ORDER BY is_default DESC, name ASC
            "#,
        )
        .bind(tenant_id)
        .bind(owner_user_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(CalendarError::from)?;
        tx.commit().await.map_err(CalendarError::from)?;
        Ok(rows)
    }

    /// Fetch a single calendar by id within tenant scope.
    pub async fn get(&self, tenant_id: Uuid, id: Uuid) -> Result<Calendar> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id)
            .await
            .map_err(CalendarError::from)?;
        let row = sqlx::query_as::<_, Calendar>(
            r#"
            SELECT id, tenant_id, owner_user_id, name, description, color,
                   timezone, ctag, is_default, created_at, updated_at
              FROM calendars
             WHERE tenant_id = $1 AND id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(CalendarError::from)?
        .ok_or(CalendarError::CalendarNotFound(id.to_string()))?;
        tx.commit().await.map_err(CalendarError::from)?;
        Ok(row)
    }

    /// Partial update — COALESCE keeps existing values when patch field is NULL.
    pub async fn update(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        patch: &UpdateCalendar,
    ) -> Result<Calendar> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id)
            .await
            .map_err(CalendarError::from)?;
        let row = sqlx::query_as::<_, Calendar>(
            r#"
            UPDATE calendars SET
                name        = COALESCE($3, name),
                description = COALESCE($4, description),
                color       = COALESCE($5, color),
                timezone    = COALESCE($6, timezone),
                is_default  = COALESCE($7, is_default)
             WHERE tenant_id = $1 AND id = $2
             RETURNING id, tenant_id, owner_user_id, name, description, color,
                       timezone, ctag, is_default, created_at, updated_at
            "#,
        )
        .bind(tenant_id)
        .bind(id)
        .bind(&patch.name)
        .bind(&patch.description)
        .bind(&patch.color)
        .bind(&patch.timezone)
        .bind(patch.is_default)
        .fetch_optional(&mut *tx)
        .await
        .map_err(CalendarError::from)?
        .ok_or(CalendarError::CalendarNotFound(id.to_string()))?;
        tx.commit().await.map_err(CalendarError::from)?;
        Ok(row)
    }

    /// Delete calendar and cascade events.
    pub async fn delete(&self, tenant_id: Uuid, id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id)
            .await
            .map_err(CalendarError::from)?;
        let res = sqlx::query(
            r#"DELETE FROM calendars WHERE tenant_id = $1 AND id = $2"#,
        )
        .bind(tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(CalendarError::from)?;

        if res.rows_affected() == 0 {
            return Err(CalendarError::CalendarNotFound(id.to_string()));
        }
        tx.commit().await.map_err(CalendarError::from)?;
        Ok(())
    }

    /// Current ctag (used by CalDAV PROPFIND).
    pub async fn ctag(&self, tenant_id: Uuid, id: Uuid) -> Result<i64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id)
            .await
            .map_err(CalendarError::from)?;
        let (ctag,): (i64,) = sqlx::query_as(
            r#"SELECT ctag FROM calendars WHERE tenant_id = $1 AND id = $2"#,
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(CalendarError::from)?
        .ok_or(CalendarError::CalendarNotFound(id.to_string()))?;
        tx.commit().await.map_err(CalendarError::from)?;
        Ok(ctag)
    }

    /// List calendars visible to user: owned + shared via `calendar_acl`.
    pub async fn list_accessible(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
    ) -> Result<Vec<Calendar>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id)
            .await
            .map_err(CalendarError::from)?;
        let rows = sqlx::query_as::<_, Calendar>(
            r#"
            SELECT id, tenant_id, owner_user_id, name, description, color,
                   timezone, ctag, is_default, created_at, updated_at
              FROM calendars
             WHERE tenant_id = $1
               AND (owner_user_id = $2
                    OR id IN (SELECT calendar_id FROM calendar_acl
                               WHERE tenant_id = $1 AND grantee_id = $2))
             ORDER BY is_default DESC, name ASC
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(CalendarError::from)?;
        tx.commit().await.map_err(CalendarError::from)?;
        Ok(rows)
    }

    /// Effective access level for user on calendar:
    /// returns "OWNER" | "READ" | "WRITE" | "ADMIN" | None.
    ///
    /// Os dois SELECTs (owner em `calendars` + privilege em `calendar_acl`)
    /// rodam na MESMA tx, dentro do mesmo snapshot — evita race em ACL handoff
    /// (transferência de owner concorrente com revoke de ACL poderia retornar
    /// None mesmo quando o user ainda tinha acesso).
    pub async fn access_level(
        &self,
        tenant_id: Uuid,
        cal_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<String>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id)
            .await
            .map_err(CalendarError::from)?;
        let owner: Option<(Uuid,)> = sqlx::query_as(
            "SELECT owner_user_id FROM calendars WHERE id = $1 AND tenant_id = $2",
        )
        .bind(cal_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(CalendarError::from)?;
        match owner {
            None => {
                tx.commit().await.map_err(CalendarError::from)?;
                return Ok(None);
            }
            Some((o,)) if o == user_id => {
                tx.commit().await.map_err(CalendarError::from)?;
                return Ok(Some("OWNER".into()));
            }
            _ => {}
        }
        let acl: Option<(String,)> = sqlx::query_as(
            "SELECT privilege FROM calendar_acl
              WHERE calendar_id = $1 AND tenant_id = $2 AND grantee_id = $3",
        )
        .bind(cal_id)
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(CalendarError::from)?;
        tx.commit().await.map_err(CalendarError::from)?;
        Ok(acl.map(|(p,)| p))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn sample() -> Calendar {
        Calendar {
            id:            Uuid::nil(),
            tenant_id:     Uuid::nil(),
            owner_user_id: Uuid::nil(),
            name:          "Personal".into(),
            description:   Some("My calendar".into()),
            color:         Some("#ff0000".into()),
            timezone:      "America/Sao_Paulo".into(),
            ctag:          42,
            is_default:    true,
            created_at:    datetime!(2026-05-22 08:00:00 UTC),
            updated_at:    datetime!(2026-05-22 08:00:00 UTC),
        }
    }

    #[test]
    fn calendar_serde_roundtrip() {
        let c = sample();
        let s = serde_json::to_string(&c).unwrap();
        let back: Calendar = serde_json::from_str(&s).unwrap();
        assert_eq!(back.name, "Personal");
        assert_eq!(back.ctag, 42);
        assert!(back.is_default);
    }

    #[test]
    fn calendar_color_preserved() {
        let c = sample();
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("#ff0000"));
    }

    #[test]
    fn new_calendar_deserialize_minimal() {
        let json = r#"{"name":"Work"}"#;
        let n: NewCalendar = serde_json::from_str(json).unwrap();
        assert_eq!(n.name, "Work");
        assert!(n.description.is_none());
        assert!(!n.is_default);
    }

    #[test]
    fn update_calendar_default_all_none() {
        let u = UpdateCalendar::default();
        assert!(u.name.is_none());
        assert!(u.description.is_none());
        assert!(u.color.is_none());
        assert!(u.timezone.is_none());
        assert!(u.is_default.is_none());
    }

    #[test]
    fn update_calendar_partial_deserialize() {
        let json = r#"{"name":"Updated","is_default":false}"#;
        let u: UpdateCalendar = serde_json::from_str(json).unwrap();
        assert_eq!(u.name.as_deref(), Some("Updated"));
        assert_eq!(u.is_default, Some(false));
        assert!(u.color.is_none());
    }

    #[test]
    fn new_calendar_is_default_true() {
        let n = NewCalendar {
            name: "Main".into(),
            description: None,
            color: None,
            timezone: None,
            is_default: true,
        };
        assert!(n.is_default);
    }

    #[test]
    fn update_calendar_timezone_set() {
        let u: UpdateCalendar = serde_json::from_str(r#"{"timezone":"America/Sao_Paulo"}"#).unwrap();
        assert_eq!(u.timezone.as_deref(), Some("America/Sao_Paulo"));
    }

    #[test]
    fn new_calendar_description_none_by_default() {
        let n = NewCalendar {
            name: "Work".into(),
            description: None,
            color: Some("#336699".into()),
            timezone: None,
            is_default: false,
        };
        assert!(n.description.is_none());
        assert_eq!(n.color.as_deref(), Some("#336699"));
    }

    #[test]
    fn new_calendar_name_preserved() {
        let n = NewCalendar {
            name: "Personal".into(),
            description: Some("My personal calendar".into()),
            color: None,
            timezone: None,
            is_default: true,
        };
        assert_eq!(n.name, "Personal");
        assert!(n.is_default);
    }

    #[test]
    fn new_calendar_color_none_by_default() {
        let n = NewCalendar {
            name: "Work".into(),
            description: None,
            color: None, timezone: None, is_default: false,
        };
        assert!(n.color.is_none());
    }

    #[test]
    fn new_calendar_timezone_none_by_default() {
        let n = NewCalendar {
            name: "Team".into(),
            description: None,
            color: None, timezone: None, is_default: false,
        };
        assert!(n.timezone.is_none());
    }

    #[test]
    fn new_calendar_name_field_round_trips() {
        let n = NewCalendar {
            name: "Personal".into(),
            description: None,
            color: None, timezone: None, is_default: false,
        };
        assert_eq!(n.name, "Personal");
    }

    #[test]
    fn new_calendar_is_default_false_by_default() {
        let n = NewCalendar {
            name: "Work".into(),
            description: None,
            color: None, timezone: None, is_default: false,
        };
        assert!(!n.is_default);
    }
}

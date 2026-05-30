//! Contact groups (distribution lists) — persistence layer.
//!
//! Tenant scoping mirrors `AddressbookRepo`: every method opens a transaction
//! via `begin_tenant_tx` so the `contact_groups` / `contact_group_members` RLS
//! policies filter on `current_setting('app.tenant_id')` ahead of the explicit
//! `WHERE tenant_id = $1` (defense-in-depth). Membership is a join table, so a
//! contact can belong to many groups; `add_member` is idempotent.

use expresso_core::{begin_tenant_tx, DbPool};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::contact::Contact;
use crate::error::{ContactsError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ContactGroup {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub owner_user_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewContactGroup {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UpdateContactGroup {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Maximum group name length — generous for a label, bounded to keep listings
/// and the unique index sane.
pub const MAX_GROUP_NAME_LEN: usize = 256;

#[derive(Clone)]
pub struct ContactGroupRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> ContactGroupRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        tenant_id: Uuid,
        owner_user_id: Uuid,
        input: NewContactGroup,
    ) -> Result<ContactGroup> {
        let name = validate_name(&input.name)?;
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, ContactGroup>(
            r#"
            INSERT INTO contact_groups (tenant_id, owner_user_id, name, description)
            VALUES ($1, $2, $3, $4)
            RETURNING *
            "#,
        )
        .bind(tenant_id)
        .bind(owner_user_id)
        .bind(name)
        .bind(input.description)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn list_for_owner(&self, tenant_id: Uuid, owner: Uuid) -> Result<Vec<ContactGroup>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query_as::<_, ContactGroup>(
            r#"SELECT * FROM contact_groups
               WHERE tenant_id = $1 AND owner_user_id = $2
               ORDER BY name"#,
        )
        .bind(tenant_id)
        .bind(owner)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn get(&self, tenant_id: Uuid, id: Uuid) -> Result<ContactGroup> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, ContactGroup>(
            r#"SELECT * FROM contact_groups WHERE tenant_id = $1 AND id = $2"#,
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn update(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        input: UpdateContactGroup,
    ) -> Result<ContactGroup> {
        if let Some(ref n) = input.name {
            validate_name(n)?;
        }
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, ContactGroup>(
            r#"
            UPDATE contact_groups
               SET name        = COALESCE($3, name),
                   description = COALESCE($4, description)
             WHERE tenant_id = $1 AND id = $2
             RETURNING *
            "#,
        )
        .bind(tenant_id)
        .bind(id)
        .bind(input.name)
        .bind(input.description)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn delete(&self, tenant_id: Uuid, id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(r#"DELETE FROM contact_groups WHERE tenant_id = $1 AND id = $2"#)
            .bind(tenant_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        if res.rows_affected() == 0 {
            return Err(ContactsError::Database(sqlx::Error::RowNotFound));
        }
        Ok(())
    }

    /// Add a contact to a group. Idempotent: a second add of the same pair is a
    /// no-op (`ON CONFLICT DO NOTHING`). Both group and contact must already
    /// exist in the tenant; a missing FK surfaces as a DB error (mapped 4xx by
    /// the handler via `ContactsError`). The membership row carries `tenant_id`
    /// so its own RLS policy holds.
    pub async fn add_member(
        &self,
        tenant_id: Uuid,
        group_id: Uuid,
        contact_id: Uuid,
    ) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        sqlx::query(
            r#"
            INSERT INTO contact_group_members (group_id, contact_id, tenant_id)
            VALUES ($1, $2, $3)
            ON CONFLICT (group_id, contact_id) DO NOTHING
            "#,
        )
        .bind(group_id)
        .bind(contact_id)
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Remove a contact from a group. Returns `RowNotFound` when the pair was
    /// not a member, so the handler can answer 404 rather than a silent 204.
    pub async fn remove_member(
        &self,
        tenant_id: Uuid,
        group_id: Uuid,
        contact_id: Uuid,
    ) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            r#"DELETE FROM contact_group_members
               WHERE tenant_id = $1 AND group_id = $2 AND contact_id = $3"#,
        )
        .bind(tenant_id)
        .bind(group_id)
        .bind(contact_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        if res.rows_affected() == 0 {
            return Err(ContactsError::Database(sqlx::Error::RowNotFound));
        }
        Ok(())
    }

    /// List the full contact rows in a group, newest membership first.
    pub async fn list_members(&self, tenant_id: Uuid, group_id: Uuid) -> Result<Vec<Contact>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query_as::<_, Contact>(
            r#"
            SELECT c.*
            FROM contacts c
            JOIN contact_group_members m ON m.contact_id = c.id
            WHERE m.tenant_id = $1 AND m.group_id = $2
            ORDER BY m.added_at DESC
            "#,
        )
        .bind(tenant_id)
        .bind(group_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }
}

/// Reject empty/whitespace-only and oversize group names; returns the trimmed
/// form so storage is canonical and the per-owner unique index is meaningful.
fn validate_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ContactsError::BadRequest(
            "group name must not be empty".into(),
        ));
    }
    if name.len() > MAX_GROUP_NAME_LEN {
        return Err(ContactsError::BadRequest(format!(
            "group name too long: {} bytes (max {MAX_GROUP_NAME_LEN})",
            name.len()
        )));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_accepts_normal() {
        assert_eq!(validate_name("Team").unwrap(), "Team");
    }

    #[test]
    fn validate_name_trims() {
        assert_eq!(validate_name("  Team  ").unwrap(), "Team");
    }

    #[test]
    fn validate_name_rejects_empty() {
        assert!(validate_name("").is_err());
    }

    #[test]
    fn validate_name_rejects_whitespace_only() {
        assert!(validate_name("   ").is_err());
    }

    #[test]
    fn validate_name_rejects_oversize() {
        let long = "x".repeat(MAX_GROUP_NAME_LEN + 1);
        assert!(validate_name(&long).is_err());
    }

    #[test]
    fn validate_name_accepts_at_cap() {
        let at_cap = "x".repeat(MAX_GROUP_NAME_LEN);
        assert!(validate_name(&at_cap).is_ok());
    }

    #[test]
    fn new_group_description_optional() {
        let g: NewContactGroup = serde_json::from_str(r#"{"name":"Team"}"#).unwrap();
        assert!(g.description.is_none());
    }

    #[test]
    fn update_group_all_optional() {
        let u: UpdateContactGroup = serde_json::from_str("{}").unwrap();
        assert!(u.name.is_none());
        assert!(u.description.is_none());
    }

    #[test]
    fn group_serde_roundtrip() {
        let json = r#"{
            "id":"00000000-0000-0000-0000-000000000000",
            "tenant_id":"00000000-0000-0000-0000-000000000000",
            "owner_user_id":"00000000-0000-0000-0000-000000000000",
            "name":"Team","description":null,
            "created_at":"2026-01-01T00:00:00Z",
            "updated_at":"2026-01-01T00:00:00Z"
        }"#;
        let g: ContactGroup = serde_json::from_str(json).unwrap();
        assert_eq!(g.name, "Team");
        let back = serde_json::to_string(&g).unwrap();
        assert!(back.contains("\"name\":\"Team\""));
    }
}

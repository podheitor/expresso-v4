//! Notebook (folder) domain model + repository.
//!
//! A notebook groups a user's notes. It is owned by one user within a tenant;
//! RLS scopes the tenant via `begin_tenant_tx` and an explicit `user_id`
//! predicate scopes ownership. Notebooks are private — there is no notebook ACL
//! (sharing is per-note via `notes_acl`).

use expresso_core::{begin_tenant_tx, DbPool};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{NotesError, Result};

/// Abuse guard. A notebook name is a single label.
const MAX_NAME_BYTES: usize = 256;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Notebook {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub color: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewNotebook {
    pub name: String,
    pub color: Option<String>,
}

/// Partial update — only present fields change. `color` is doubly-optional so a
/// caller can clear it (`"color": null`) vs. leave it (absent).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateNotebook {
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<Option<String>>,
}

pub struct NotebookRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> NotebookRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, tenant: Uuid, user: Uuid, n: NewNotebook) -> Result<Notebook> {
        let name = validate_name(&n.name)?;
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, Notebook>(
            r#"INSERT INTO notes_notebooks (tenant_id, user_id, name, color)
               VALUES ($1, $2, $3, $4)
               RETURNING *"#,
        )
        .bind(tenant)
        .bind(user)
        .bind(name)
        .bind(n.color.as_deref())
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// List a user's notebooks, newest first.
    pub async fn list(&self, tenant: Uuid, user: Uuid) -> Result<Vec<Notebook>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows = sqlx::query_as::<_, Notebook>(
            r#"SELECT * FROM notes_notebooks
                WHERE tenant_id = $1 AND user_id = $2
                ORDER BY created_at DESC"#,
        )
        .bind(tenant)
        .bind(user)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Fetch one of the user's notebooks. 404 if absent or not theirs —
    /// notebooks are private, so a foreign id is indistinguishable from a
    /// missing one.
    pub async fn get(&self, tenant: Uuid, user: Uuid, id: Uuid) -> Result<Notebook> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, Notebook>(
            r#"SELECT * FROM notes_notebooks
                WHERE tenant_id = $1 AND user_id = $2 AND id = $3"#,
        )
        .bind(tenant)
        .bind(user)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(NotesError::NotebookNotFound(id))?;
        tx.commit().await?;
        Ok(row)
    }

    /// Apply a partial update to the user's notebook. 404 if absent or foreign.
    pub async fn update(
        &self,
        tenant: Uuid,
        user: Uuid,
        id: Uuid,
        u: UpdateNotebook,
    ) -> Result<Notebook> {
        let current = self.get(tenant, user, id).await?;
        let name = match &u.name {
            Some(n) => validate_name(n)?.to_owned(),
            None => current.name,
        };
        let color = match u.color {
            Some(c) => c,
            None => current.color,
        };
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, Notebook>(
            r#"UPDATE notes_notebooks
                  SET name = $4, color = $5
                WHERE tenant_id = $1 AND user_id = $2 AND id = $3
                RETURNING *"#,
        )
        .bind(tenant)
        .bind(user)
        .bind(id)
        .bind(name)
        .bind(color)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Delete the user's notebook. Notes attached to it detach (the FK is
    /// `ON DELETE SET NULL`), not deleted. 404 if absent or foreign.
    pub async fn delete(&self, tenant: Uuid, user: Uuid, id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let res = sqlx::query(
            "DELETE FROM notes_notebooks WHERE tenant_id = $1 AND user_id = $2 AND id = $3",
        )
        .bind(tenant)
        .bind(user)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if res.rows_affected() == 0 {
            return Err(NotesError::NotebookNotFound(id));
        }
        tx.commit().await?;
        Ok(())
    }

    /// True if the user owns this notebook — used to validate a note's target
    /// notebook before (re)attaching it. A foreign/absent notebook returns
    /// false so the caller can 400 rather than create a dangling reference.
    pub async fn owns(&self, tenant: Uuid, user: Uuid, id: Uuid) -> Result<bool> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM notes_notebooks WHERE tenant_id = $1 AND user_id = $2 AND id = $3",
        )
        .bind(tenant)
        .bind(user)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row.is_some())
    }
}

fn validate_name(raw: &str) -> Result<&str> {
    let n = raw.trim();
    if n.is_empty() {
        return Err(NotesError::BadRequest("notebook name is required".into()));
    }
    if n.len() > MAX_NAME_BYTES {
        return Err(NotesError::BadRequest("notebook name too long".into()));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_trimmed_and_bounded() {
        assert_eq!(validate_name("  Work ").unwrap(), "Work");
        let long = "x".repeat(MAX_NAME_BYTES + 1);
        assert!(validate_name(&long).is_err());
    }

    #[test]
    fn empty_name_rejected() {
        assert!(validate_name("   ").is_err());
        assert!(validate_name("").is_err());
    }
}

//! Notes domain model + repository.
//!
//! A note belongs to one user within a tenant. RLS scopes the tenant via
//! `begin_tenant_tx`; the repo additionally filters `user_id` so a user only
//! ever sees their own notes (no sharing in this slice).

use expresso_core::{begin_tenant_tx, DbPool};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{NotesError, Result};

/// Abuse guards. Title is a single line; body covers a long memo.
const MAX_TITLE_BYTES: usize = 1024;
const MAX_BODY_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Note {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub title: String,
    pub body: String,
    pub color: Option<String>,
    pub pinned: bool,
    pub archived: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewNote {
    pub title: Option<String>,
    pub body: Option<String>,
    pub color: Option<String>,
    pub pinned: Option<bool>,
}

/// Partial update — only present fields change. `color` is doubly-optional so a
/// caller can clear it (`"color": null`) vs. leave it (absent).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateNote {
    pub title: Option<String>,
    pub body: Option<String>,
    #[serde(default)]
    pub color: Option<Option<String>>,
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
}

pub struct NoteRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> NoteRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, tenant: Uuid, user: Uuid, n: NewNote) -> Result<Note> {
        let title = validate_title(n.title.as_deref().unwrap_or(""))?;
        let body = validate_body(n.body.as_deref().unwrap_or(""))?;
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, Note>(
            r#"INSERT INTO notes (tenant_id, user_id, title, body, color, pinned)
               VALUES ($1, $2, $3, $4, $5, $6)
               RETURNING *"#,
        )
        .bind(tenant)
        .bind(user)
        .bind(title)
        .bind(body)
        .bind(n.color.as_deref())
        .bind(n.pinned.unwrap_or(false))
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// List a user's notes. `archived = false` returns the live board; `true`
    /// the archive. Pinned first, then newest-updated.
    pub async fn list(&self, tenant: Uuid, user: Uuid, archived: bool) -> Result<Vec<Note>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows = sqlx::query_as::<_, Note>(
            r#"SELECT * FROM notes
                WHERE tenant_id = $1 AND user_id = $2 AND archived = $3
                ORDER BY pinned DESC, updated_at DESC"#,
        )
        .bind(tenant)
        .bind(user)
        .bind(archived)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn get(&self, tenant: Uuid, user: Uuid, id: Uuid) -> Result<Note> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, Note>(
            r#"SELECT * FROM notes WHERE tenant_id = $1 AND user_id = $2 AND id = $3"#,
        )
        .bind(tenant)
        .bind(user)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(NotesError::NoteNotFound(id))?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn update(&self, tenant: Uuid, user: Uuid, id: Uuid, u: UpdateNote) -> Result<Note> {
        let current = self.get(tenant, user, id).await?;
        let title = match &u.title {
            Some(t) => validate_title(t)?.to_owned(),
            None => current.title,
        };
        let body = match &u.body {
            Some(b) => validate_body(b)?.to_owned(),
            None => current.body,
        };
        let color = match u.color {
            Some(c) => c,
            None => current.color,
        };
        let pinned = u.pinned.unwrap_or(current.pinned);
        let archived = u.archived.unwrap_or(current.archived);

        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, Note>(
            r#"UPDATE notes
                  SET title = $4, body = $5, color = $6, pinned = $7, archived = $8
                WHERE tenant_id = $1 AND user_id = $2 AND id = $3
                RETURNING *"#,
        )
        .bind(tenant)
        .bind(user)
        .bind(id)
        .bind(title)
        .bind(body)
        .bind(color)
        .bind(pinned)
        .bind(archived)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn delete(&self, tenant: Uuid, user: Uuid, id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let res =
            sqlx::query("DELETE FROM notes WHERE tenant_id = $1 AND user_id = $2 AND id = $3")
                .bind(tenant)
                .bind(user)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        if res.rows_affected() == 0 {
            return Err(NotesError::NoteNotFound(id));
        }
        tx.commit().await?;
        Ok(())
    }
}

fn validate_title(raw: &str) -> Result<&str> {
    let t = raw.trim();
    if t.len() > MAX_TITLE_BYTES {
        return Err(NotesError::BadRequest("title too long".into()));
    }
    Ok(t)
}

fn validate_body(raw: &str) -> Result<&str> {
    if raw.len() > MAX_BODY_BYTES {
        return Err(NotesError::BadRequest("body too long".into()));
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_trimmed_and_bounded() {
        assert_eq!(validate_title("  hi ").unwrap(), "hi");
        let long = "x".repeat(MAX_TITLE_BYTES + 1);
        assert!(validate_title(&long).is_err());
    }

    #[test]
    fn body_bounded() {
        assert!(validate_body("ok").is_ok());
        let long = "x".repeat(MAX_BODY_BYTES + 1);
        assert!(validate_body(&long).is_err());
    }

    #[test]
    fn empty_title_and_body_ok() {
        assert_eq!(validate_title("").unwrap(), "");
        assert_eq!(validate_body("").unwrap(), "");
    }
}

//! Notes domain model + repository.
//!
//! A note belongs to one user within a tenant. RLS scopes the tenant via
//! `begin_tenant_tx`. `list` is owner-scoped (a user's own board); `get`/
//! `update`/`delete` resolve by id and let the handler gate access via
//! [`NoteRepo::access_level`] (owner or `notes_acl` grant), so shared notes are
//! reachable.

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
    /// Owning notebook (folder), or `None` for a loose note.
    pub notebook_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// A note shared with the caller, plus the caller's granted privilege.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SharedNote {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub title: String,
    pub body: String,
    pub color: Option<String>,
    pub pinned: bool,
    pub archived: bool,
    pub notebook_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// The caller's grant: READ | WRITE | ADMIN.
    pub privilege: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewNote {
    pub title: Option<String>,
    pub body: Option<String>,
    pub color: Option<String>,
    pub pinned: Option<bool>,
    /// Optional notebook to file the note under at creation. The handler
    /// validates ownership before persisting.
    pub notebook_id: Option<Uuid>,
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
    /// Doubly-optional: `null` detaches from any notebook, a uuid (re)files it,
    /// absent leaves it. The handler validates ownership of a target notebook.
    #[serde(default)]
    pub notebook_id: Option<Option<Uuid>>,
}

/// How [`NoteRepo::list`] filters by notebook membership.
#[derive(Debug, Clone, Copy)]
pub enum NotebookFilter {
    /// All notes regardless of notebook.
    Any,
    /// Only loose notes (no notebook).
    Loose,
    /// Only notes filed under this notebook.
    In(Uuid),
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
            r#"INSERT INTO notes (tenant_id, user_id, title, body, color, pinned, notebook_id)
               VALUES ($1, $2, $3, $4, $5, $6, $7)
               RETURNING *"#,
        )
        .bind(tenant)
        .bind(user)
        .bind(title)
        .bind(body)
        .bind(n.color.as_deref())
        .bind(n.pinned.unwrap_or(false))
        .bind(n.notebook_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// List a user's notes. `archived = false` returns the live board; `true`
    /// the archive. `notebook` filters by notebook membership. Pinned first,
    /// then newest-updated.
    pub async fn list(
        &self,
        tenant: Uuid,
        user: Uuid,
        archived: bool,
        notebook: NotebookFilter,
    ) -> Result<Vec<Note>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        // One query, branched by filter. The `$4 IS NULL` arm keeps "any
        // notebook" and "this notebook" on the same prepared statement; the
        // loose-only case ($4 = nil sentinel) needs its own predicate.
        let rows = match notebook {
            NotebookFilter::Any => {
                sqlx::query_as::<_, Note>(
                    r#"SELECT * FROM notes
                        WHERE tenant_id = $1 AND user_id = $2 AND archived = $3
                        ORDER BY pinned DESC, updated_at DESC"#,
                )
                .bind(tenant)
                .bind(user)
                .bind(archived)
                .fetch_all(&mut *tx)
                .await?
            }
            NotebookFilter::Loose => {
                sqlx::query_as::<_, Note>(
                    r#"SELECT * FROM notes
                        WHERE tenant_id = $1 AND user_id = $2 AND archived = $3
                          AND notebook_id IS NULL
                        ORDER BY pinned DESC, updated_at DESC"#,
                )
                .bind(tenant)
                .bind(user)
                .bind(archived)
                .fetch_all(&mut *tx)
                .await?
            }
            NotebookFilter::In(nb) => {
                sqlx::query_as::<_, Note>(
                    r#"SELECT * FROM notes
                        WHERE tenant_id = $1 AND user_id = $2 AND archived = $3
                          AND notebook_id = $4
                        ORDER BY pinned DESC, updated_at DESC"#,
                )
                .bind(tenant)
                .bind(user)
                .bind(archived)
                .bind(nb)
                .fetch_all(&mut *tx)
                .await?
            }
        };
        tx.commit().await?;
        Ok(rows)
    }

    /// List a user's notes carrying `tag`, pinned first then newest-updated.
    /// `archived` selects the live board vs. the archive, matching [`list`].
    pub async fn list_by_tag(
        &self,
        tenant: Uuid,
        user: Uuid,
        archived: bool,
        tag: &str,
    ) -> Result<Vec<Note>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows = sqlx::query_as::<_, Note>(
            r#"SELECT n.* FROM notes n
                 JOIN notes_tags t ON t.note_id = n.id AND t.tenant_id = n.tenant_id
                WHERE n.tenant_id = $1 AND n.user_id = $2 AND n.archived = $3
                  AND t.tag = $4
                ORDER BY n.pinned DESC, n.updated_at DESC"#,
        )
        .bind(tenant)
        .bind(user)
        .bind(archived)
        .bind(tag)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Case-insensitive substring search over a user's own notes (title + body),
    /// pinned first then newest-updated. `%`/`_`/`\` in `query` are escaped so a
    /// literal search works. `limit` is clamped by the caller. Mirrors the REST
    /// search other services expose (contacts/calendar/drive).
    pub async fn search(
        &self,
        tenant: Uuid,
        user: Uuid,
        query: &str,
        limit: i64,
    ) -> Result<Vec<Note>> {
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows = sqlx::query_as::<_, Note>(
            r#"SELECT * FROM notes
                WHERE tenant_id = $1 AND user_id = $2
                  AND (title ILIKE $3 ESCAPE '\' OR body ILIKE $3 ESCAPE '\')
                ORDER BY pinned DESC, updated_at DESC
                LIMIT $4"#,
        )
        .bind(tenant)
        .bind(user)
        .bind(&pattern)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// List notes shared *with* `user` (a `notes_acl` grant exists), newest
    /// first. Each carries the granted `privilege` so the UI can show read-only
    /// vs editable. Own notes are excluded — those come from [`list`](Self::list).
    pub async fn list_shared(&self, tenant: Uuid, user: Uuid) -> Result<Vec<SharedNote>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows = sqlx::query_as::<_, SharedNote>(
            r#"SELECT n.id, n.tenant_id, n.user_id, n.title, n.body, n.color,
                      n.pinned, n.archived, n.notebook_id, n.created_at, n.updated_at, a.privilege
                 FROM notes n
                 JOIN notes_acl a
                   ON a.note_id = n.id AND a.tenant_id = n.tenant_id
                WHERE n.tenant_id = $1 AND a.grantee_id = $2
                ORDER BY n.updated_at DESC"#,
        )
        .bind(tenant)
        .bind(user)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Fetch a note by id within the tenant — NOT owner-filtered. Access is
    /// gated by the handler via [`access_level`](Self::access_level), so this
    /// returns shared notes too. 404 if absent in the tenant.
    pub async fn get(&self, tenant: Uuid, id: Uuid) -> Result<Note> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row =
            sqlx::query_as::<_, Note>(r#"SELECT * FROM notes WHERE tenant_id = $1 AND id = $2"#)
                .bind(tenant)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(NotesError::NoteNotFound(id))?;
        tx.commit().await?;
        Ok(row)
    }

    /// Apply a partial update. Not owner-filtered (the handler enforces WRITE+
    /// access); 404 if the note is absent in the tenant.
    pub async fn update(&self, tenant: Uuid, id: Uuid, u: UpdateNote) -> Result<Note> {
        let current = self.get(tenant, id).await?;
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
        let notebook_id = match u.notebook_id {
            Some(nb) => nb,
            None => current.notebook_id,
        };

        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, Note>(
            r#"UPDATE notes
                  SET title = $3, body = $4, color = $5, pinned = $6, archived = $7,
                      notebook_id = $8
                WHERE tenant_id = $1 AND id = $2
                RETURNING *"#,
        )
        .bind(tenant)
        .bind(id)
        .bind(title)
        .bind(body)
        .bind(color)
        .bind(pinned)
        .bind(archived)
        .bind(notebook_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Delete a note by id. Not owner-filtered (the handler enforces WRITE+
    /// access); 404 when nothing was removed.
    pub async fn delete(&self, tenant: Uuid, id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let res = sqlx::query("DELETE FROM notes WHERE tenant_id = $1 AND id = $2")
            .bind(tenant)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if res.rows_affected() == 0 {
            return Err(NotesError::NoteNotFound(id));
        }
        tx.commit().await?;
        Ok(())
    }

    /// The caller's access to a note: `Some("OWNER")` when they own it, else the
    /// granted privilege from `notes_acl` (`READ`/`WRITE`/`ADMIN`), else `None`
    /// (no access, or note absent). Owner + grant lookups share one tx snapshot.
    pub async fn access_level(
        &self,
        tenant: Uuid,
        note_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<String>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let owner: Option<(Uuid,)> =
            sqlx::query_as("SELECT user_id FROM notes WHERE id = $1 AND tenant_id = $2")
                .bind(note_id)
                .bind(tenant)
                .fetch_optional(&mut *tx)
                .await?;
        match owner {
            None => {
                tx.commit().await?;
                return Ok(None);
            }
            Some((o,)) if o == user_id => {
                tx.commit().await?;
                return Ok(Some("OWNER".into()));
            }
            _ => {}
        }
        let acl: Option<(String,)> = sqlx::query_as(
            "SELECT privilege FROM notes_acl
              WHERE note_id = $1 AND tenant_id = $2 AND grantee_id = $3",
        )
        .bind(note_id)
        .bind(tenant)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(acl.map(|(p,)| p))
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

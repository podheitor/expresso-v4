//! Note tags (labels) domain + repository.
//!
//! Tags are free-form labels attached to a note, many-to-many. They live in
//! `notes_tags` and are scoped by RLS via `begin_tenant_tx`. Access control is
//! the note's — the handler gates read/write through [`NoteRepo`] before
//! touching tags, so this repo trusts its caller on authorization.

use std::collections::BTreeSet;

use expresso_core::{begin_tenant_tx, DbPool};
use serde::Serialize;
use uuid::Uuid;

use crate::error::{NotesError, Result};

/// Tag length bound, mirroring the `notes_tags` CHECK constraint.
const MAX_TAG_BYTES: usize = 64;
/// Cap tags per note to keep a single PUT bounded.
const MAX_TAGS_PER_NOTE: usize = 64;

/// One tag and how many of the caller's notes carry it.
#[derive(Debug, Serialize, PartialEq, Eq, sqlx::FromRow)]
pub struct TagCount {
    pub tag: String,
    pub count: i64,
}

pub struct NoteTagRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> NoteTagRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Replace a note's entire tag set with `tags` (deduped, trimmed). An empty
    /// set clears all tags. Returns the stored set, sorted.
    pub async fn set(&self, tenant: Uuid, note_id: Uuid, tags: &[String]) -> Result<Vec<String>> {
        let clean = normalize(tags)?;
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        sqlx::query("DELETE FROM notes_tags WHERE tenant_id = $1 AND note_id = $2")
            .bind(tenant)
            .bind(note_id)
            .execute(&mut *tx)
            .await?;
        for tag in &clean {
            sqlx::query(
                "INSERT INTO notes_tags (note_id, tenant_id, tag) VALUES ($1, $2, $3)
                 ON CONFLICT (note_id, tag) DO NOTHING",
            )
            .bind(note_id)
            .bind(tenant)
            .bind(tag)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(clean)
    }

    /// List a note's tags, sorted.
    pub async fn list(&self, tenant: Uuid, note_id: Uuid) -> Result<Vec<String>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT tag FROM notes_tags
              WHERE tenant_id = $1 AND note_id = $2 ORDER BY tag",
        )
        .bind(tenant)
        .bind(note_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows.into_iter().map(|(t,)| t).collect())
    }

    /// Count how many of `user`'s notes carry each tag. Scoped to the user's own
    /// notes via a join (`notes_tags` has no `user_id`). Ordered by descending
    /// count, then tag — the natural "most-used first" view. Empty when the user
    /// has no tagged notes.
    pub async fn count_by_tag(&self, tenant: Uuid, user: Uuid) -> Result<Vec<TagCount>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows: Vec<TagCount> = sqlx::query_as(
            "SELECT t.tag AS tag, COUNT(*) AS count FROM notes_tags t
               JOIN notes n ON n.id = t.note_id AND n.tenant_id = t.tenant_id
              WHERE t.tenant_id = $1 AND n.user_id = $2
              GROUP BY t.tag
              ORDER BY count DESC, t.tag",
        )
        .bind(tenant)
        .bind(user)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Rename tag `old` to `new` across all of `user`'s notes. Doubles as merge:
    /// if a note already carries `new`, the row collapses (ON CONFLICT) instead
    /// of erroring. Scoped to the user's own notes via a join. Returns the number
    /// of notes whose `old` tag was removed. No-op when `old == new`.
    pub async fn rename_across_notes(
        &self,
        tenant: Uuid,
        user: Uuid,
        old: &str,
        new: &str,
    ) -> Result<u64> {
        let new_clean = validate_one(new)?;
        if old.trim() == new_clean {
            return Ok(0);
        }
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        // Add `new` to every owned note that has `old` (idempotent on conflict).
        sqlx::query(
            "INSERT INTO notes_tags (note_id, tenant_id, tag)
             SELECT t.note_id, t.tenant_id, $4 FROM notes_tags t
               JOIN notes n ON n.id = t.note_id AND n.tenant_id = t.tenant_id
              WHERE t.tenant_id = $1 AND n.user_id = $2 AND t.tag = $3
             ON CONFLICT (note_id, tag) DO NOTHING",
        )
        .bind(tenant)
        .bind(user)
        .bind(old.trim())
        .bind(new_clean)
        .execute(&mut *tx)
        .await?;
        // Drop `old` from those notes.
        let removed = sqlx::query(
            "DELETE FROM notes_tags t
              USING notes n
              WHERE t.note_id = n.id AND t.tenant_id = n.tenant_id
                AND t.tenant_id = $1 AND n.user_id = $2 AND t.tag = $3",
        )
        .bind(tenant)
        .bind(user)
        .bind(old.trim())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(removed)
    }
}

/// Validate a single tag (trim, non-empty, bounded) → owned String.
fn validate_one(raw: &str) -> Result<String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(NotesError::BadRequest("tag must not be empty".into()));
    }
    if t.len() > MAX_TAG_BYTES {
        return Err(NotesError::BadRequest("tag too long".into()));
    }
    Ok(t.to_owned())
}

/// Trim, drop blanks, dedupe, and bound. Rejects an over-long tag or too many
/// tags so a bad request fails before touching the DB.
fn normalize(tags: &[String]) -> Result<Vec<String>> {
    let mut set = BTreeSet::new();
    for raw in tags {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        if t.len() > MAX_TAG_BYTES {
            return Err(NotesError::BadRequest(format!("tag too long: {t}")));
        }
        set.insert(t.to_owned());
    }
    if set.len() > MAX_TAGS_PER_NOTE {
        return Err(NotesError::BadRequest("too many tags".into()));
    }
    Ok(set.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_one_trims_and_bounds() {
        assert_eq!(validate_one("  work ").unwrap(), "work");
        assert!(validate_one("   ").is_err());
        assert!(validate_one(&"x".repeat(MAX_TAG_BYTES + 1)).is_err());
    }

    #[test]
    fn normalize_trims_dedupes_sorts() {
        let got =
            normalize(&[" work ".into(), "urgent".into(), "work".into(), "  ".into()]).unwrap();
        assert_eq!(got, vec!["urgent".to_string(), "work".to_string()]);
    }

    #[test]
    fn normalize_rejects_overlong_tag() {
        let long = "x".repeat(MAX_TAG_BYTES + 1);
        assert!(normalize(&[long]).is_err());
    }

    #[test]
    fn normalize_empty_is_empty() {
        assert!(normalize(&[]).unwrap().is_empty());
        assert!(normalize(&["   ".into()]).unwrap().is_empty());
    }
}

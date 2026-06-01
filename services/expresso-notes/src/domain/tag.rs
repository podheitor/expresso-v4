//! Note tags (labels) domain + repository.
//!
//! Tags are free-form labels attached to a note, many-to-many. They live in
//! `notes_tags` and are scoped by RLS via `begin_tenant_tx`. Access control is
//! the note's — the handler gates read/write through [`NoteRepo`] before
//! touching tags, so this repo trusts its caller on authorization.

use std::collections::BTreeSet;

use expresso_core::{begin_tenant_tx, DbPool};
use uuid::Uuid;

use crate::error::{NotesError, Result};

/// Tag length bound, mirroring the `notes_tags` CHECK constraint.
const MAX_TAG_BYTES: usize = 64;
/// Cap tags per note to keep a single PUT bounded.
const MAX_TAGS_PER_NOTE: usize = 64;

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

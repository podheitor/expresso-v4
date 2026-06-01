//! Note version history domain + repository.
//!
//! Each content edit snapshots the note's prior title/body/color into
//! `notes_versions`. `version_no` is assigned per note as `max + 1` inside the
//! snapshot transaction, so concurrent edits get distinct numbers. Restore reads
//! a snapshot's content back; the handler then applies it via [`NoteRepo`]
//! (which itself snapshots, so a restore is also versioned — an undo is
//! reversible). RLS scopes the tenant via `begin_tenant_tx`.

use expresso_core::{begin_tenant_tx, DbPool};
use serde::Serialize;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{NotesError, Result};

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct NoteVersion {
    pub id: Uuid,
    pub note_id: Uuid,
    pub tenant_id: Uuid,
    pub version_no: i32,
    pub title: String,
    pub body: String,
    pub color: Option<String>,
    pub edited_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// The content fields a snapshot captures, taken from the live note before an
/// edit. View state (pinned/archived/notebook) is excluded by design.
#[derive(Debug, Clone)]
pub struct NoteSnapshot {
    pub title: String,
    pub body: String,
    pub color: Option<String>,
}

pub struct NoteVersionRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> NoteVersionRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Snapshot a note's prior content as the next version. `edited_by` is the
    /// user making the edit that supersedes this content.
    pub async fn snapshot(
        &self,
        tenant: Uuid,
        note_id: Uuid,
        snap: &NoteSnapshot,
        edited_by: Uuid,
    ) -> Result<NoteVersion> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, NoteVersion>(
            r#"INSERT INTO notes_versions
                   (note_id, tenant_id, version_no, title, body, color, edited_by)
               SELECT $1, $2,
                      COALESCE(MAX(version_no), 0) + 1, $3, $4, $5, $6
                 FROM notes_versions WHERE note_id = $1 AND tenant_id = $2
               RETURNING *"#,
        )
        .bind(note_id)
        .bind(tenant)
        .bind(&snap.title)
        .bind(&snap.body)
        .bind(snap.color.as_deref())
        .bind(edited_by)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// List a note's versions, newest first.
    pub async fn list(&self, tenant: Uuid, note_id: Uuid) -> Result<Vec<NoteVersion>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows = sqlx::query_as::<_, NoteVersion>(
            r#"SELECT * FROM notes_versions
                WHERE tenant_id = $1 AND note_id = $2
                ORDER BY version_no DESC"#,
        )
        .bind(tenant)
        .bind(note_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Fetch one version by its number within a note. 404 if absent.
    pub async fn get(&self, tenant: Uuid, note_id: Uuid, version_no: i32) -> Result<NoteVersion> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, NoteVersion>(
            r#"SELECT * FROM notes_versions
                WHERE tenant_id = $1 AND note_id = $2 AND version_no = $3"#,
        )
        .bind(tenant)
        .bind(note_id)
        .bind(version_no)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(NotesError::VersionNotFound(version_no))?;
        tx.commit().await?;
        Ok(row)
    }
}

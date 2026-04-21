//! Drive file metadata repository.
//!
//! Files + folders tracked in `drive_files`. Content bytes live on the
//! filesystem under `<data_root>/<storage_key>`. Soft-delete via `deleted_at`
//! → items remain visible under /drive/trash until purged.

use expresso_core::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{DriveError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DriveFile {
    pub id:            Uuid,
    pub tenant_id:     Uuid,
    pub owner_user_id: Uuid,
    pub parent_id:     Option<Uuid>,
    pub name:          String,
    pub kind:          String,
    pub mime_type:     Option<String>,
    pub size_bytes:    i64,
    pub sha256:        Option<String>,
    pub storage_key:   Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:    OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at:    OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub deleted_at:    Option<OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct NewFile {
    pub tenant_id:     Uuid,
    pub owner_user_id: Uuid,
    pub parent_id:     Option<Uuid>,
    pub name:          String,
    pub kind:          String,
    pub mime_type:     Option<String>,
    pub size_bytes:    i64,
    pub sha256:        Option<String>,
    pub storage_key:   Option<String>,
}

pub struct FileRepo<'a> {
    pool: &'a DbPool,
}

const SELECT_COLS: &str = "id, tenant_id, owner_user_id, parent_id, name, kind, \
    mime_type, size_bytes, sha256, storage_key, created_at, updated_at, deleted_at";

impl<'a> FileRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    pub async fn insert(&self, f: &NewFile) -> Result<DriveFile> {
        let sql = format!(
            "INSERT INTO drive_files \
             (tenant_id, owner_user_id, parent_id, name, kind, mime_type, \
              size_bytes, sha256, storage_key) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) \
             RETURNING {SELECT_COLS}"
        );
        let row: DriveFile = sqlx::query_as(&sql)
            .bind(f.tenant_id)
            .bind(f.owner_user_id)
            .bind(f.parent_id)
            .bind(&f.name)
            .bind(&f.kind)
            .bind(&f.mime_type)
            .bind(f.size_bytes)
            .bind(&f.sha256)
            .bind(&f.storage_key)
            .fetch_one(self.pool)
            .await
            .map_err(map_conflict)?;
        Ok(row)
    }

    pub async fn get(&self, tenant_id: Uuid, id: Uuid) -> Result<DriveFile> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_files \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id)
            .fetch_optional(self.pool).await?;
        row.ok_or(DriveError::NotFound(id))
    }

    pub async fn list_children(
        &self,
        tenant_id: Uuid,
        parent_id: Option<Uuid>,
    ) -> Result<Vec<DriveFile>> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_files \
             WHERE tenant_id = $1 \
               AND deleted_at IS NULL \
               AND parent_id IS NOT DISTINCT FROM $2 \
             ORDER BY kind DESC, lower(name)"
        );
        let rows: Vec<DriveFile> = sqlx::query_as(&sql)
            .bind(tenant_id).bind(parent_id)
            .fetch_all(self.pool).await?;
        Ok(rows)
    }

    pub async fn list_trash(&self, tenant_id: Uuid) -> Result<Vec<DriveFile>> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_files \
             WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
             ORDER BY deleted_at DESC"
        );
        let rows: Vec<DriveFile> = sqlx::query_as(&sql)
            .bind(tenant_id)
            .fetch_all(self.pool).await?;
        Ok(rows)
    }


    pub async fn find_by_name(
        &self,
        tenant_id: Uuid,
        parent_id: Option<Uuid>,
        name:      &str,
    ) -> Result<Option<DriveFile>> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_files \
             WHERE tenant_id = $1 \
               AND parent_id IS NOT DISTINCT FROM $2 \
               AND name = $3 \
               AND deleted_at IS NULL"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(tenant_id).bind(parent_id).bind(name)
            .fetch_optional(self.pool).await?;
        Ok(row)
    }

    /// Swap storage_key + size + sha + mime in place. Used quando upload
    /// substitui versão corrente (histórico já foi arquivado em drive_file_versions).
    pub async fn update_content(
        &self,
        tenant_id:   Uuid,
        id:          Uuid,
        storage_key: &str,
        size_bytes:  i64,
        sha256:      Option<&str>,
        mime_type:   Option<&str>,
    ) -> Result<DriveFile> {
        let sql = format!(
            "UPDATE drive_files \
                SET storage_key = $3, size_bytes = $4, sha256 = $5, \
                    mime_type = $6, updated_at = now() \
              WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL \
              RETURNING {SELECT_COLS}"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id)
            .bind(storage_key).bind(size_bytes).bind(sha256).bind(mime_type)
            .fetch_optional(self.pool).await?;
        row.ok_or(DriveError::NotFound(id))
    }

    pub async fn soft_delete(&self, tenant_id: Uuid, id: Uuid) -> Result<u64> {
        let r = sqlx::query(
            "UPDATE drive_files SET deleted_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL",
        )
        .bind(id).bind(tenant_id)
        .execute(self.pool).await?;
        Ok(r.rows_affected())
    }

    /// Clear deleted_at → move back to live tree. Conflict if a live
    /// sibling already holds the name (unique partial index raises 23505).
    pub async fn restore(&self, tenant_id: Uuid, id: Uuid) -> Result<DriveFile> {
        let sql = format!(
            "UPDATE drive_files SET deleted_at = NULL, updated_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NOT NULL \
             RETURNING {SELECT_COLS}"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id)
            .fetch_optional(self.pool).await
            .map_err(map_conflict)?;
        row.ok_or(DriveError::NotFound(id))
    }

    /// Hard delete → caller must unlink the storage blob. Only soft-deleted
    /// rows may be purged to avoid accidental data loss.
    pub async fn purge(&self, tenant_id: Uuid, id: Uuid) -> Result<Option<String>> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "DELETE FROM drive_files \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NOT NULL \
             RETURNING storage_key",
        )
        .bind(id).bind(tenant_id)
        .fetch_optional(self.pool).await?;
        Ok(row.and_then(|(k,)| k))
    }
}

fn map_conflict(e: sqlx::Error) -> DriveError {
    if let sqlx::Error::Database(db) = &e {
        if db.code().as_deref() == Some("23505") {
            return DriveError::Conflict("a sibling with this name already exists".into());
        }
    }
    DriveError::Database(e)
}

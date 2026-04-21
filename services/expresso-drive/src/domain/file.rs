//! Drive file metadata repository.
//!
//! Phase 3 scaffold: files + folders tracked in `drive_files`. Content bytes
//! live on the filesystem under `<data_root>/<storage_key>`. Soft-delete via
//! `deleted_at`.

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

impl<'a> FileRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    pub async fn insert(&self, f: &NewFile) -> Result<DriveFile> {
        let row: DriveFile = sqlx::query_as(
            r#"INSERT INTO drive_files
               (tenant_id, owner_user_id, parent_id, name, kind, mime_type,
                size_bytes, sha256, storage_key)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
               RETURNING id, tenant_id, owner_user_id, parent_id, name, kind,
                         mime_type, size_bytes, sha256, storage_key,
                         created_at, updated_at, deleted_at"#,
        )
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
        let row: Option<DriveFile> = sqlx::query_as(
            r#"SELECT id, tenant_id, owner_user_id, parent_id, name, kind,
                      mime_type, size_bytes, sha256, storage_key,
                      created_at, updated_at, deleted_at
                 FROM drive_files
                WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL"#,
        )
        .bind(id).bind(tenant_id)
        .fetch_optional(self.pool).await?;
        row.ok_or(DriveError::NotFound(id))
    }

    pub async fn list_children(
        &self,
        tenant_id: Uuid,
        parent_id: Option<Uuid>,
    ) -> Result<Vec<DriveFile>> {
        let rows: Vec<DriveFile> = sqlx::query_as(
            r#"SELECT id, tenant_id, owner_user_id, parent_id, name, kind,
                      mime_type, size_bytes, sha256, storage_key,
                      created_at, updated_at, deleted_at
                 FROM drive_files
                WHERE tenant_id = $1
                  AND deleted_at IS NULL
                  AND parent_id IS NOT DISTINCT FROM $2
                ORDER BY kind DESC, lower(name)"#,
        )
        .bind(tenant_id).bind(parent_id)
        .fetch_all(self.pool).await?;
        Ok(rows)
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
}

fn map_conflict(e: sqlx::Error) -> DriveError {
    if let sqlx::Error::Database(db) = &e {
        if db.code().as_deref() == Some("23505") {
            return DriveError::Conflict("a sibling with this name already exists".into());
        }
    }
    DriveError::Database(e)
}

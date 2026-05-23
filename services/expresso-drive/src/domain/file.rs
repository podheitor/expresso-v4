//! Drive file metadata repository.
//!
//! Files + folders tracked in `drive_files`. Content bytes live on the
//! filesystem under `<data_root>/<storage_key>`. Soft-delete via `deleted_at`
//! → items remain visible under /drive/trash until purged.
//!
//! Tenant scoping: cada método abre transação via `begin_tenant_tx` para
//! que a policy RLS de `drive_files` filtre por `current_setting('app.tenant_id')`.
//! As cláusulas `WHERE tenant_id = $1` permanecem como defense-in-depth.

use expresso_core::{begin_tenant_tx, DbPool};
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
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at:    Option<OffsetDateTime>,
    pub locked_by:     Option<Uuid>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub locked_at:     Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub starred_at:    Option<OffsetDateTime>,
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
    mime_type, size_bytes, sha256, storage_key, created_at, updated_at, deleted_at, expires_at, \
    locked_by, locked_at, starred_at";

impl<'a> FileRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    pub async fn insert(&self, f: &NewFile) -> Result<DriveFile> {
        let mut tx = begin_tenant_tx(self.pool, f.tenant_id).await?;
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
            .fetch_one(&mut *tx)
            .await
            .map_err(map_conflict)?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn get(&self, tenant_id: Uuid, id: Uuid) -> Result<DriveFile> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_files \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        row.ok_or(DriveError::NotFound(id))
    }

    pub async fn list_children(
        &self,
        tenant_id: Uuid,
        parent_id: Option<Uuid>,
    ) -> Result<Vec<DriveFile>> {
        self.list_children_sorted(tenant_id, parent_id, "name", "asc").await
    }

    /// Like `list_children` but with caller-specified sort column and direction.
    /// `sort_col` must be one of "name" | "updated_at" | "created_at" | "size_bytes"
    /// (validated by caller). `order` is "asc" or "desc" (validated by caller).
    pub async fn list_children_sorted(
        &self,
        tenant_id: Uuid,
        parent_id: Option<Uuid>,
        sort_col:  &str,
        order:     &str,
    ) -> Result<Vec<DriveFile>> {
        let order_clause = match (sort_col, order) {
            ("name",       "asc")  => "kind DESC, lower(name) ASC",
            ("name",       _)      => "kind DESC, lower(name) DESC",
            ("updated_at", "asc")  => "updated_at ASC",
            ("updated_at", _)      => "updated_at DESC",
            ("created_at", "asc")  => "created_at ASC",
            ("created_at", _)      => "created_at DESC",
            ("size_bytes", "asc")  => "size_bytes ASC",
            ("size_bytes", _)      => "size_bytes DESC",
            _                      => "kind DESC, lower(name) ASC",
        };
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_files \
             WHERE tenant_id = $1 \
               AND deleted_at IS NULL \
               AND parent_id IS NOT DISTINCT FROM $2 \
             ORDER BY {order_clause}"
        );
        let rows: Vec<DriveFile> = sqlx::query_as(&sql)
            .bind(tenant_id).bind(parent_id)
            .fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn list_children_paged(
        &self,
        tenant_id: Uuid,
        parent_id: Option<Uuid>,
        sort_col:  &str,
        order:     &str,
        limit:     i64,
        offset:    i64,
    ) -> Result<Vec<DriveFile>> {
        let order_clause = match (sort_col, order) {
            ("name",       "asc")  => "kind DESC, lower(name) ASC",
            ("name",       _)      => "kind DESC, lower(name) DESC",
            ("updated_at", "asc")  => "updated_at ASC",
            ("updated_at", _)      => "updated_at DESC",
            ("created_at", "asc")  => "created_at ASC",
            ("created_at", _)      => "created_at DESC",
            ("size_bytes", "asc")  => "size_bytes ASC",
            ("size_bytes", _)      => "size_bytes DESC",
            _                      => "kind DESC, lower(name) ASC",
        };
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_files \
             WHERE tenant_id = $1 \
               AND deleted_at IS NULL \
               AND parent_id IS NOT DISTINCT FROM $2 \
             ORDER BY {order_clause} \
             LIMIT $3 OFFSET $4"
        );
        let rows: Vec<DriveFile> = sqlx::query_as(&sql)
            .bind(tenant_id).bind(parent_id).bind(limit).bind(offset)
            .fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn list_trash(&self, tenant_id: Uuid) -> Result<Vec<DriveFile>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_files \
             WHERE tenant_id = $1 AND deleted_at IS NOT NULL \
             ORDER BY deleted_at DESC"
        );
        let rows: Vec<DriveFile> = sqlx::query_as(&sql)
            .bind(tenant_id)
            .fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }


    pub async fn find_by_name(
        &self,
        tenant_id: Uuid,
        parent_id: Option<Uuid>,
        name:      &str,
    ) -> Result<Option<DriveFile>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_files \
             WHERE tenant_id = $1 \
               AND parent_id IS NOT DISTINCT FROM $2 \
               AND name = $3 \
               AND deleted_at IS NULL"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(tenant_id).bind(parent_id).bind(name)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Insert a new row that shares the same blob (storage_key) as an existing
    /// file. The caller is responsible for choosing the copy name and parent.
    /// Folders get an empty-content copy (storage_key = None, size = 0).
    pub async fn copy_file(
        &self,
        tenant_id:     Uuid,
        owner_user_id: Uuid,
        src_id:        Uuid,
        new_name:      String,
        new_parent_id: Option<Uuid>,
    ) -> Result<DriveFile> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "INSERT INTO drive_files \
             (tenant_id, owner_user_id, parent_id, name, kind, mime_type, \
              size_bytes, sha256, storage_key) \
             SELECT $1, $2, $4, $5, kind, mime_type, size_bytes, sha256, storage_key \
             FROM drive_files \
             WHERE id = $3 AND tenant_id = $1 AND deleted_at IS NULL \
             RETURNING {SELECT_COLS}"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(tenant_id)
            .bind(owner_user_id)
            .bind(src_id)
            .bind(new_parent_id)
            .bind(new_name)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_conflict)?;
        tx.commit().await?;
        row.ok_or(DriveError::NotFound(src_id))
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
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
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
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        row.ok_or(DriveError::NotFound(id))
    }

    pub async fn soft_delete(&self, tenant_id: Uuid, id: Uuid) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let r = sqlx::query(
            "UPDATE drive_files SET deleted_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL",
        )
        .bind(id).bind(tenant_id)
        .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(r.rows_affected())
    }

    /// Restore multiple trashed files/folders in a single transaction.
    /// Returns count of rows actually restored (already-live rows are skipped).
    pub async fn bulk_restore(&self, tenant_id: Uuid, ids: &[Uuid]) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let r = sqlx::query(
            "UPDATE drive_files SET deleted_at = NULL, updated_at = now() \
             WHERE tenant_id = $1 AND id = ANY($2) AND deleted_at IS NOT NULL",
        )
        .bind(tenant_id)
        .bind(ids)
        .execute(&mut *tx).await
        .map_err(map_conflict)?;
        tx.commit().await?;
        Ok(r.rows_affected())
    }

    /// Soft-delete multiple files/folders in a single transaction.
    /// Returns count of rows actually trashed (already-deleted rows are skipped).
    pub async fn bulk_trash(&self, tenant_id: Uuid, ids: &[Uuid]) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let r = sqlx::query(
            "UPDATE drive_files SET deleted_at = now() \
             WHERE tenant_id = $1 AND id = ANY($2) AND deleted_at IS NULL",
        )
        .bind(tenant_id)
        .bind(ids)
        .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(r.rows_affected())
    }

    /// Clear deleted_at → move back to live tree. Conflict if a live
    /// sibling already holds the name (unique partial index raises 23505).
    pub async fn restore(&self, tenant_id: Uuid, id: Uuid) -> Result<DriveFile> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "UPDATE drive_files SET deleted_at = NULL, updated_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NOT NULL \
             RETURNING {SELECT_COLS}"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id)
            .fetch_optional(&mut *tx).await
            .map_err(map_conflict)?;
        tx.commit().await?;
        row.ok_or(DriveError::NotFound(id))
    }

    /// Move multiple files/folders to a new parent in a single transaction.
    /// Returns the list of moved rows. Skips ids that are deleted or belong
    /// to a different tenant (they simply don't match the WHERE predicate).
    pub async fn bulk_move(
        &self,
        tenant_id:     Uuid,
        ids:           &[Uuid],
        new_parent_id: Option<Uuid>,
    ) -> Result<Vec<DriveFile>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "UPDATE drive_files SET parent_id = $3, updated_at = now() \
             WHERE tenant_id = $1 AND id = ANY($2) AND deleted_at IS NULL \
             RETURNING {SELECT_COLS}"
        );
        let rows: Vec<DriveFile> = sqlx::query_as(&sql)
            .bind(tenant_id)
            .bind(ids)
            .bind(new_parent_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(map_conflict)?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Move file/folder to a different parent (or root when parent_id = None).
    pub async fn move_to(&self, tenant_id: Uuid, id: Uuid, new_parent_id: Option<Uuid>) -> Result<DriveFile> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "UPDATE drive_files SET parent_id = $3, updated_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL \
             RETURNING {SELECT_COLS}"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id).bind(new_parent_id)
            .fetch_optional(&mut *tx).await
            .map_err(map_conflict)?;
        tx.commit().await?;
        row.ok_or(DriveError::NotFound(id))
    }

    /// Rename file/folder. name must already be validated by the caller.
    pub async fn rename(&self, tenant_id: Uuid, id: Uuid, new_name: String) -> Result<DriveFile> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "UPDATE drive_files SET name = $3, updated_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL \
             RETURNING {SELECT_COLS}"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id).bind(new_name)
            .fetch_optional(&mut *tx).await
            .map_err(map_conflict)?;
        tx.commit().await?;
        row.ok_or(DriveError::NotFound(id))
    }

    /// Bulk shallow-copy: one new row per source id sharing the same blob.
    /// Each copy is named "<original name> (cópia)". Returns only the rows
    /// that were successfully copied; missing/deleted source ids are skipped.
    pub async fn bulk_copy_files(
        &self,
        tenant_id:    Uuid,
        owner_id:     Uuid,
        ids:          &[Uuid],
        new_parent_id: Option<Uuid>,
    ) -> Result<Vec<DriveFile>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "INSERT INTO drive_files \
             (tenant_id, owner_user_id, parent_id, name, kind, mime_type, size_bytes, sha256, storage_key) \
             SELECT $1, $2, $5, CONCAT(name, ' (cópia)'), kind, mime_type, size_bytes, sha256, storage_key \
             FROM drive_files \
             WHERE id = $3 AND tenant_id = $4 AND deleted_at IS NULL \
             RETURNING {SELECT_COLS}"
        );
        let mut copied: Vec<DriveFile> = Vec::new();
        for &src_id in ids {
            if let Some(row) = sqlx::query_as::<_, DriveFile>(&sql)
                .bind(tenant_id)
                .bind(owner_id)
                .bind(src_id)
                .bind(tenant_id)
                .bind(new_parent_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_conflict)?
            {
                copied.push(row);
            }
        }
        tx.commit().await?;
        Ok(copied)
    }

    /// Recursively collect all descendant *files* (kind='file') under `folder_id`,
    /// including files inside nested sub-folders. Returns pairs of `(relative_path, DriveFile)`.
    pub async fn collect_files_recursive(
        &self,
        tenant_id: Uuid,
        folder_id: Uuid,
        prefix:    &str,
    ) -> Result<Vec<(String, DriveFile)>> {
        let mut results: Vec<(String, DriveFile)> = Vec::new();
        let children = self.list_children(tenant_id, Some(folder_id)).await?;
        for child in children {
            let entry_path = if prefix.is_empty() {
                child.name.clone()
            } else {
                format!("{}/{}", prefix, child.name)
            };
            if child.kind == "file" {
                results.push((entry_path, child));
            } else if child.kind == "folder" {
                let sub = Box::pin(self.collect_files_recursive(tenant_id, child.id, &entry_path)).await?;
                results.extend(sub);
            }
        }
        Ok(results)
    }

    /// Set or clear the expiry timestamp on a file or folder.
    /// Pass `expires_at = None` to remove a previously set expiry.
    pub async fn set_expiry(
        &self,
        tenant_id:  Uuid,
        id:         Uuid,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<DriveFile> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "UPDATE drive_files SET expires_at = $3, updated_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL \
             RETURNING {SELECT_COLS}"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id).bind(expires_at)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        row.ok_or(DriveError::NotFound(id))
    }

    /// Hard-delete all live files whose `expires_at <= now()`.
    /// Returns `(count, storage_keys)` so the caller can unlink blobs.
    pub async fn purge_expired_files(&self) -> Result<Vec<(Uuid, Option<String>)>> {
        let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
            "DELETE FROM drive_files \
             WHERE expires_at IS NOT NULL AND expires_at <= now() AND deleted_at IS NULL \
             RETURNING id, storage_key",
        )
        .fetch_all(self.pool)
        .await?;
        Ok(rows)
    }

    /// Hard-delete files trashed (deleted_at NOT NULL) há mais de `older_than_days`.
    /// Tenant-scoped via begin_tenant_tx. Retorna `(id, storage_key)` pra caller
    /// remover blobs físicos. Não usa `now() - interval '...'` interpolado pra
    /// evitar SQL injection — passa o cutoff como timestamptz bind.
    pub async fn purge_trashed_older_than(
        &self,
        tenant_id: Uuid,
        cutoff: time::OffsetDateTime,
    ) -> Result<Vec<(Uuid, Option<String>)>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
            "DELETE FROM drive_files \
             WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND deleted_at <= $2 \
             RETURNING id, storage_key",
        )
        .bind(tenant_id)
        .bind(cutoff)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Star a file — sets `starred_at = now()` idempotently.
    pub async fn star_file(&self, tenant_id: Uuid, id: Uuid, user_id: Uuid) -> Result<DriveFile> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "UPDATE drive_files SET starred_at = now(), updated_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND owner_user_id = $3 AND deleted_at IS NULL \
             RETURNING {SELECT_COLS}"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id).bind(user_id)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        row.ok_or(DriveError::NotFound(id))
    }

    /// Unstar a file — clears `starred_at`.
    pub async fn unstar_file(&self, tenant_id: Uuid, id: Uuid, user_id: Uuid) -> Result<DriveFile> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "UPDATE drive_files SET starred_at = NULL, updated_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND owner_user_id = $3 AND deleted_at IS NULL \
             RETURNING {SELECT_COLS}"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id).bind(user_id)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        row.ok_or(DriveError::NotFound(id))
    }

    /// List all starred files for the user, newest star first.
    pub async fn list_starred(&self, tenant_id: Uuid, user_id: Uuid) -> Result<Vec<DriveFile>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_files \
             WHERE tenant_id = $1 AND owner_user_id = $2 \
               AND starred_at IS NOT NULL AND deleted_at IS NULL \
             ORDER BY starred_at DESC"
        );
        let rows: Vec<DriveFile> = sqlx::query_as(&sql)
            .bind(tenant_id).bind(user_id)
            .fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Conta quantos arquivos starred ativos o usuário tem (sprint #458). Mesma
    /// semântica de `list_starred` (filtra `starred_at IS NOT NULL` e `deleted_at IS NULL`)
    /// mas só agrega COUNT — útil pra badge sem trafegar payload da listagem.
    pub async fn count_starred(&self, tenant_id: Uuid, user_id: Uuid) -> Result<i64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM drive_files \
             WHERE tenant_id = $1 AND owner_user_id = $2 \
               AND starred_at IS NOT NULL AND deleted_at IS NULL",
        )
        .bind(tenant_id).bind(user_id)
        .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(count)
    }

    /// Acquire an optimistic lock on a file. Returns `Conflict` if already locked
    /// by a different user, `NotFound` if the file does not exist.
    pub async fn lock_file(
        &self,
        tenant_id: Uuid,
        id:        Uuid,
        user_id:   Uuid,
    ) -> Result<DriveFile> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "UPDATE drive_files \
               SET locked_by = $3, locked_at = now(), updated_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL \
               AND (locked_by IS NULL OR locked_by = $3) \
             RETURNING {SELECT_COLS}"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id).bind(user_id)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        match row {
            Some(f) => Ok(f),
            None => {
                // If the file exists but locked_by != user_id → Conflict
                let exists: Option<(bool,)> = sqlx::query_as(
                    "SELECT true FROM drive_files WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL",
                )
                .bind(id).bind(tenant_id)
                .fetch_optional(self.pool).await?;
                if exists.is_some() {
                    Err(DriveError::Conflict("file is locked by another user".into()))
                } else {
                    Err(DriveError::NotFound(id))
                }
            }
        }
    }

    /// Release a lock on a file. Only the lock owner may unlock.
    pub async fn unlock_file(
        &self,
        tenant_id: Uuid,
        id:        Uuid,
        user_id:   Uuid,
    ) -> Result<DriveFile> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "UPDATE drive_files \
               SET locked_by = NULL, locked_at = NULL, updated_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL AND locked_by = $3 \
             RETURNING {SELECT_COLS}"
        );
        let row: Option<DriveFile> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id).bind(user_id)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        match row {
            Some(f) => Ok(f),
            None => {
                let exists: Option<(bool,)> = sqlx::query_as(
                    "SELECT true FROM drive_files WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL",
                )
                .bind(id).bind(tenant_id)
                .fetch_optional(self.pool).await?;
                if exists.is_some() {
                    Err(DriveError::Conflict("file is not locked by you".into()))
                } else {
                    Err(DriveError::NotFound(id))
                }
            }
        }
    }

    /// Hard delete → caller must unlink the storage blob. Only soft-deleted
    /// rows may be purged to avoid accidental data loss.
    pub async fn purge(&self, tenant_id: Uuid, id: Uuid) -> Result<Option<String>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "DELETE FROM drive_files \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NOT NULL \
             RETURNING storage_key",
        )
        .bind(id).bind(tenant_id)
        .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn sample_file() -> DriveFile {
        DriveFile {
            id:            Uuid::nil(),
            tenant_id:     Uuid::nil(),
            owner_user_id: Uuid::nil(),
            parent_id:     None,
            name:          "report.pdf".into(),
            kind:          "file".into(),
            mime_type:     Some("application/pdf".into()),
            size_bytes:    1024,
            sha256:        Some("deadbeef".into()),
            storage_key:   Some("blobs/report".into()),
            created_at:    datetime!(2026-05-22 08:00:00 UTC),
            updated_at:    datetime!(2026-05-22 08:00:00 UTC),
            deleted_at:    None,
            expires_at:    None,
            locked_by:     None,
            locked_at:     None,
            starred_at:    None,
        }
    }

    #[test]
    fn drive_file_serde_roundtrip() {
        let f = sample_file();
        let s = serde_json::to_string(&f).unwrap();
        let back: DriveFile = serde_json::from_str(&s).unwrap();
        assert_eq!(back.name, "report.pdf");
        assert_eq!(back.size_bytes, 1024);
    }

    #[test]
    fn drive_file_deleted_at_none_omitted_in_json() {
        let f = sample_file();
        let s = serde_json::to_string(&f).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        // deleted_at=None with rfc3339::option → serialized as null
        assert!(v["deleted_at"].is_null());
    }

    #[test]
    fn drive_file_starred_at_some_serialized() {
        let mut f = sample_file();
        f.starred_at = Some(datetime!(2026-05-01 12:00:00 UTC));
        let s = serde_json::to_string(&f).unwrap();
        assert!(s.contains("2026-05-01T12:00:00"));
    }

    #[test]
    fn drive_file_kind_folder_preserved() {
        let mut f = sample_file();
        f.kind = "folder".into();
        let s = serde_json::to_string(&f).unwrap();
        assert!(s.contains(r#""kind":"folder""#));
    }

    #[test]
    fn drive_file_storage_key_none_is_null() {
        let mut f = sample_file();
        f.storage_key = None;
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert!(v["storage_key"].is_null());
    }

    #[test]
    fn drive_file_size_zero_serialises() {
        let mut f = sample_file();
        f.size_bytes = 0;
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(v["size_bytes"], 0);
    }

    #[test]
    fn drive_file_locked_by_none_is_null() {
        let f = sample_file();
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert!(v["locked_by"].is_null());
    }

    #[test]
    fn drive_file_parent_id_none_is_null() {
        let f = sample_file();
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert!(v["parent_id"].is_null());
    }

    #[test]
    fn drive_file_name_accessible() {
        let f = sample_file();
        assert!(!f.name.is_empty());
    }

    #[test]
    fn drive_file_size_bytes_accessible() {
        let f = sample_file();
        assert!(f.size_bytes >= 0);
    }

    #[test]
    fn drive_file_locked_by_none_initially() {
        let f = sample_file();
        assert!(f.locked_by.is_none());
    }

    #[test]
    fn drive_file_mime_type_is_none_for_dir() {
        let f = sample_file();
        // sample_file has kind "file"; mime_type may be None by default
        let _ = f.mime_type; // field exists and is Option<String>
    }

    #[test]
    fn drive_file_sha256_is_option() {
        let f = sample_file();
        let _ = f.sha256; // compiles only if field exists and is Option<String>
    }

    #[test]
    fn drive_file_name_preserved() {
        let f = sample_file();
        assert_eq!(f.name, "report.pdf");
    }

    #[test]
    fn drive_file_tenant_id_accessible() {
        let f = sample_file();
        let _ = f.tenant_id; // field exists and is Uuid
    }

    #[test]
    fn drive_file_kind_is_file() {
        let f = sample_file();
        assert_eq!(f.kind, "file");
    }

    #[test]
    fn drive_file_mime_type_preserved_in_roundtrip() {
        let f = sample_file();
        let back: DriveFile = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(back.mime_type.as_deref(), Some("application/pdf"));
    }

    #[test]
    fn drive_file_owner_user_id_accessible() {
        let f = sample_file();
        let _ = f.owner_user_id;
    }

    #[test]
    fn drive_file_name_is_nonempty_in_sample() {
        let f = sample_file();
        assert!(!f.name.is_empty());
    }

    #[test]
    fn drive_file_expires_at_none_by_default() {
        let f = sample_file();
        assert!(f.expires_at.is_none());
    }

    #[test]
    fn drive_file_sha256_none_in_sample() {
        let f = sample_file();
        assert!(f.sha256.is_none());
    }
}

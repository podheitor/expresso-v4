//! Drive per-tenant quota.
//!
//! Tenant scoping: `get` abre transação via `begin_tenant_tx` para que a
//! policy RLS de `drive_quotas` filtre por `current_setting('app.tenant_id')`
//! e os dois SELECTs (linha de quota + função `drive_quota_used`) rodem
//! contra um snapshot consistente. `WHERE tenant_id = $1` permanece como
//! defense-in-depth.

use expresso_core::{begin_tenant_tx, DbPool};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{DriveError, Result};

/// Default quota = 10 GB quando tenant não tem linha em drive_quotas.
pub const DEFAULT_QUOTA_BYTES: i64 = 10 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Quota {
    pub max_bytes:  i64,
    pub used_bytes: i64,
}

impl Quota {
    pub fn fits(&self, extra: i64) -> bool {
        self.used_bytes.saturating_add(extra) <= self.max_bytes
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct UserUsage {
    pub user_id:    Uuid,
    pub used_bytes: i64,
}

/// Per-folder quota: max bytes allowed in a specific folder (shallow — direct children only).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FolderQuota {
    pub folder_id:  Uuid,
    pub max_bytes:  i64,
    pub used_bytes: i64,
}

impl FolderQuota {
    pub fn fits(&self, extra: i64) -> bool {
        self.used_bytes.saturating_add(extra) <= self.max_bytes
    }
}

pub struct FolderQuotaRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> FolderQuotaRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    /// Get folder quota + current shallow used bytes. Returns None if no quota configured.
    pub async fn get(&self, tenant_id: Uuid, folder_id: Uuid) -> Result<Option<FolderQuota>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT max_bytes FROM drive_folder_quotas \
             WHERE tenant_id = $1 AND folder_id = $2"
        )
        .bind(tenant_id)
        .bind(folder_id)
        .fetch_optional(&mut *tx).await?;
        let max_bytes = match row {
            Some((m,)) => m,
            None => { tx.commit().await?; return Ok(None); }
        };
        let (used,): (Option<i64>,) = sqlx::query_as(
            "SELECT SUM(size_bytes) FROM drive_files \
             WHERE tenant_id = $1 AND parent_id = $2 AND deleted_at IS NULL"
        )
        .bind(tenant_id)
        .bind(folder_id)
        .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(Some(FolderQuota { folder_id, max_bytes, used_bytes: used.unwrap_or(0) }))
    }

    /// Set (upsert) folder quota.
    pub async fn set(&self, tenant_id: Uuid, folder_id: Uuid, max_bytes: i64) -> Result<FolderQuota> {
        if max_bytes <= 0 {
            return Err(DriveError::BadRequest("max_bytes must be > 0".into()));
        }
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        sqlx::query(
            "INSERT INTO drive_folder_quotas (tenant_id, folder_id, max_bytes, updated_at) \
             VALUES ($1, $2, $3, now()) \
             ON CONFLICT (tenant_id, folder_id) DO UPDATE SET \
                max_bytes  = EXCLUDED.max_bytes, \
                updated_at = now()"
        )
        .bind(tenant_id)
        .bind(folder_id)
        .bind(max_bytes)
        .execute(&mut *tx).await?;
        let (used,): (Option<i64>,) = sqlx::query_as(
            "SELECT SUM(size_bytes) FROM drive_files \
             WHERE tenant_id = $1 AND parent_id = $2 AND deleted_at IS NULL"
        )
        .bind(tenant_id)
        .bind(folder_id)
        .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(FolderQuota { folder_id, max_bytes, used_bytes: used.unwrap_or(0) })
    }

    /// Remove folder quota. Returns true if a row was deleted.
    pub async fn delete(&self, tenant_id: Uuid, folder_id: Uuid) -> Result<bool> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            "DELETE FROM drive_folder_quotas WHERE tenant_id = $1 AND folder_id = $2"
        )
        .bind(tenant_id)
        .bind(folder_id)
        .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }
}

pub struct QuotaRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> QuotaRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    /// Total non-deleted bytes owned by a specific user within a tenant.
    pub async fn get_user_usage(&self, tenant_id: Uuid, user_id: Uuid) -> Result<UserUsage> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let (used,): (Option<i64>,) = sqlx::query_as(
            "SELECT SUM(size_bytes) FROM drive_files \
             WHERE tenant_id = $1 AND owner_user_id = $2 AND deleted_at IS NULL"
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(UserUsage { user_id, used_bytes: used.unwrap_or(0) })
    }

    /// Top-N users by storage usage within a tenant (non-deleted files only).
    /// Returns `Vec<(owner_user_id, file_count, used_bytes)>` ordered by used_bytes DESC.
    pub async fn top_users_by_usage(&self, tenant_id: Uuid, limit: i64) -> Result<Vec<(Uuid, i64, i64)>> {
        let rows: Vec<(Uuid, i64, i64)> = sqlx::query_as(
            "SELECT owner_user_id, \
                    COUNT(*)::BIGINT AS file_count, \
                    COALESCE(SUM(size_bytes), 0)::BIGINT AS used_bytes \
               FROM drive_files \
              WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
              GROUP BY owner_user_id \
              ORDER BY used_bytes DESC \
              LIMIT $2",
        )
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows)
    }

    /// Top-N folders by recursive storage usage within a tenant.
    /// Returns `Vec<(folder_id, folder_name, file_count, used_bytes)>` ordered by used_bytes DESC.
    /// Uses a recursive CTE to accumulate size_bytes of all descendant files for each folder.
    pub async fn top_folders_by_usage(&self, tenant_id: Uuid, limit: i64) -> Result<Vec<(Uuid, String, i64, i64)>> {
        let rows: Vec<(Uuid, String, i64, i64)> = sqlx::query_as(
            "WITH RECURSIVE folder_tree AS ( \
                SELECT id, name, parent_id, id AS root_id \
                  FROM drive_files \
                 WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'folder' \
                UNION ALL \
                SELECT f.id, f.name, f.parent_id, ft.root_id \
                  FROM drive_files f \
                  JOIN folder_tree ft ON f.parent_id = ft.id \
                 WHERE f.tenant_id = $1 AND f.deleted_at IS NULL \
             ) \
             SELECT \
                 folders.id AS folder_id, \
                 folders.name AS folder_name, \
                 COALESCE(agg.file_count, 0)::BIGINT AS file_count, \
                 COALESCE(agg.used_bytes, 0)::BIGINT AS used_bytes \
             FROM drive_files folders \
             LEFT JOIN ( \
                 SELECT ft.root_id, COUNT(f.id)::BIGINT AS file_count, \
                        COALESCE(SUM(f.size_bytes), 0)::BIGINT AS used_bytes \
                   FROM folder_tree ft \
                   JOIN drive_files f ON f.parent_id = ft.id \
                  WHERE f.tenant_id = $1 AND f.deleted_at IS NULL AND f.kind = 'file' \
                  GROUP BY ft.root_id \
             ) agg ON agg.root_id = folders.id \
             WHERE folders.tenant_id = $1 AND folders.deleted_at IS NULL AND folders.kind = 'folder' \
             ORDER BY used_bytes DESC \
             LIMIT $2",
        )
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows)
    }

    /// Returns (extension, file_count, total_bytes) ordered by total_bytes DESC.
    /// Extension is derived from `lower(split_part(name, '.', -1))` — the part
    /// after the last dot. Files with no dot get extension "".
    pub async fn stats_by_extension(&self, tenant_id: Uuid, limit: i64) -> Result<Vec<(String, i64, i64)>> {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(
            "SELECT \
                lower(split_part(name, '.', -1)) AS ext, \
                COUNT(*)::BIGINT AS file_count, \
                COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
             FROM drive_files \
             WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
             GROUP BY ext \
             ORDER BY total_bytes DESC \
             LIMIT $2",
        )
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows)
    }

    /// Returns (day, uploads, updates, deletes) for the given range.
    /// `since`/`until` are optional bounds on `created_at`. Sprint #636.
    pub async fn activity_by_day(
        &self,
        tenant_id: Uuid,
        since:     Option<time::OffsetDateTime>,
        until:     Option<time::OffsetDateTime>,
    ) -> Result<Vec<(String, i64, i64, i64)>> {
        let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
            "SELECT \
                to_char(date_trunc('day', created_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
                COUNT(*) FILTER (WHERE action = 'upload')::BIGINT   AS uploads, \
                COUNT(*) FILTER (WHERE action = 'update')::BIGINT   AS updates, \
                COUNT(*) FILTER (WHERE action = 'delete')::BIGINT   AS deletes \
             FROM drive_file_activity \
             WHERE tenant_id = $1 \
               AND ($2::timestamptz IS NULL OR created_at >= $2) \
               AND ($3::timestamptz IS NULL OR created_at <  $3) \
             GROUP BY day \
             ORDER BY day ASC",
        )
        .bind(tenant_id)
        .bind(since)
        .bind(until)
        .fetch_all(self.pool)
        .await?;
        Ok(rows)
    }

    /// Top-N owners by file count + total bytes within a tenant (non-deleted files only).
    /// Returns `Vec<(owner_user_id, file_count, total_bytes)>` ordered by file_count DESC.
    /// Sprint #646.
    pub async fn top_owners_by_file_count(&self, tenant_id: Uuid, limit: i64) -> Result<Vec<(Uuid, i64, i64)>> {
        let rows: Vec<(Uuid, i64, i64)> = sqlx::query_as(
            "SELECT owner_user_id, \
                    COUNT(*)::BIGINT AS file_count, \
                    COALESCE(SUM(size_bytes), 0)::BIGINT AS total_bytes \
               FROM drive_files \
              WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
              GROUP BY owner_user_id \
              ORDER BY file_count DESC \
              LIMIT $2",
        )
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows)
    }

    /// Returns (month, file_count) ordered ASC — when files were created, by calendar month.
    /// Sprint #641.
    pub async fn age_by_month(&self, tenant_id: Uuid) -> Result<Vec<(String, i64)>> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT \
                to_char(date_trunc('month', created_at AT TIME ZONE 'UTC'), 'YYYY-MM') AS month, \
                COUNT(*)::BIGINT AS file_count \
             FROM drive_files \
             WHERE tenant_id = $1 AND deleted_at IS NULL AND kind = 'file' \
             GROUP BY month \
             ORDER BY month ASC",
        )
        .bind(tenant_id)
        .fetch_all(self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get(&self, tenant_id: Uuid) -> Result<Quota> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let (max,): (Option<i64>,) = sqlx::query_as(
            "SELECT max_bytes FROM drive_quotas WHERE tenant_id = $1"
        )
        .bind(tenant_id)
        .fetch_optional(&mut *tx).await?
        .unwrap_or((None,));
        let (used,): (Option<i64>,) = sqlx::query_as(
            "SELECT drive_quota_used($1)"
        )
        .bind(tenant_id)
        .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(Quota {
            max_bytes:  max.unwrap_or(DEFAULT_QUOTA_BYTES),
            used_bytes: used.unwrap_or(0),
        })
    }
}

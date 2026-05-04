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

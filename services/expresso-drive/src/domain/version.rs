//! Drive file version history.
//!
//! Versões antigas ficam em `drive_file_versions`; blob permanece em disco
//! sob `<data_root>/<storage_key>`. Linha viva em `drive_files` sempre aponta
//! para a versão atual.
//!
//! Tenant scoping: cada método abre transação via `begin_tenant_tx` para
//! que a policy RLS de `drive_file_versions` filtre por
//! `current_setting('app.tenant_id')`. As cláusulas `WHERE tenant_id = $1`
//! permanecem como defense-in-depth. `next_no` agora requer `tenant_id`
//! para evitar race condition entre tenants (UUID collision-free na
//! prática, mas o filtro fecha o canal e habilita RLS).

use expresso_core::{begin_tenant_tx, DbPool};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FileVersion {
    pub id:          Uuid,
    pub file_id:     Uuid,
    pub tenant_id:   Uuid,
    pub version_no:  i32,
    pub storage_key: String,
    pub size_bytes:  i64,
    pub sha256:      Option<String>,
    pub mime_type:   Option<String>,
    pub created_by:  Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:  OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct NewVersion<'a> {
    pub file_id:     Uuid,
    pub tenant_id:   Uuid,
    pub version_no:  i32,
    pub storage_key: &'a str,
    pub size_bytes:  i64,
    pub sha256:      Option<&'a str>,
    pub mime_type:   Option<&'a str>,
    pub created_by:  Uuid,
}

pub struct VersionRepo<'a> {
    pool: &'a DbPool,
}

const SELECT_COLS: &str = "id, file_id, tenant_id, version_no, storage_key, \
    size_bytes, sha256, mime_type, created_by, created_at";

impl<'a> VersionRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    /// Próximo número de versão para um arquivo (1 se nunca versionado).
    pub async fn next_no(&self, tenant_id: Uuid, file_id: Uuid) -> Result<i32> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let (current,): (Option<i32>,) = sqlx::query_as(
            "SELECT MAX(version_no) FROM drive_file_versions \
             WHERE tenant_id = $1 AND file_id = $2"
        )
        .bind(tenant_id).bind(file_id)
        .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(current.unwrap_or(0) + 1)
    }

    pub async fn insert(&self, v: &NewVersion<'_>) -> Result<FileVersion> {
        let mut tx = begin_tenant_tx(self.pool, v.tenant_id).await?;
        let sql = format!(
            "INSERT INTO drive_file_versions \
             (file_id, tenant_id, version_no, storage_key, size_bytes, sha256, mime_type, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
             RETURNING {SELECT_COLS}"
        );
        let row = sqlx::query_as(&sql)
            .bind(v.file_id)
            .bind(v.tenant_id)
            .bind(v.version_no)
            .bind(v.storage_key)
            .bind(v.size_bytes)
            .bind(v.sha256)
            .bind(v.mime_type)
            .bind(v.created_by)
            .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn list(&self, tenant_id: Uuid, file_id: Uuid) -> Result<Vec<FileVersion>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_file_versions \
             WHERE tenant_id = $1 AND file_id = $2 \
             ORDER BY version_no DESC"
        );
        let rows = sqlx::query_as(&sql)
            .bind(tenant_id).bind(file_id)
            .fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn get(&self, tenant_id: Uuid, file_id: Uuid, version_no: i32) -> Result<Option<FileVersion>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_file_versions \
             WHERE tenant_id = $1 AND file_id = $2 AND version_no = $3"
        );
        let row = sqlx::query_as(&sql)
            .bind(tenant_id).bind(file_id).bind(version_no)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }
}

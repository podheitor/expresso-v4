//! Drive resumable uploads repository — tus.io server state.
//!
//! Tenant scoping: métodos que carregam um `tenant_id` (insert/get/
//! advance_offset/delete) abrem transação via `begin_tenant_tx` para que
//! a policy RLS de `drive_uploads` filtre por
//! `current_setting('app.tenant_id')`. Os filtros `WHERE tenant_id = $1`
//! permanecem como defense-in-depth.
//!
//! `list_expired_keys` e `purge_expired` são cross-tenant por design
//! (rodam dentro do GC task em main.rs, sem contexto de tenant) e
//! continuam usando o pool diretamente — o deployment depende de uma
//! role com BYPASSRLS para esse caminho.

use expresso_core::{begin_tenant_tx, DbPool};
use serde::Serialize;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct UploadSession {
    pub id:            Uuid,
    pub tenant_id:     Uuid,
    pub owner_user_id: Uuid,
    pub parent_id:     Option<Uuid>,
    pub name:          String,
    pub mime_type:     Option<String>,
    pub total_size:    i64,
    pub offset_bytes:  i64,
    pub storage_key:   String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:    OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at:    OffsetDateTime,
}

impl UploadSession {
    /// True when `expires_at` is in the past (tus.io expiration extension).
    pub fn is_expired(&self) -> bool {
        self.expires_at <= OffsetDateTime::now_utc()
    }
}

pub struct NewUpload<'a> {
    pub tenant_id:     Uuid,
    pub owner_user_id: Uuid,
    pub parent_id:     Option<Uuid>,
    pub name:          &'a str,
    pub mime_type:     Option<&'a str>,
    pub total_size:    i64,
    pub storage_key:   &'a str,
}

pub struct UploadRepo<'a> { pool: &'a DbPool }

const COLS: &str = "id, tenant_id, owner_user_id, parent_id, name, mime_type, \
    total_size, offset_bytes, storage_key, created_at, expires_at";

impl<'a> UploadRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    pub async fn insert(&self, n: &NewUpload<'_>) -> Result<UploadSession> {
        let mut tx = begin_tenant_tx(self.pool, n.tenant_id).await?;
        let sql = format!(
            "INSERT INTO drive_uploads (tenant_id, owner_user_id, parent_id, name, \
                mime_type, total_size, storage_key) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) \
             RETURNING {COLS}"
        );
        let row: UploadSession = sqlx::query_as(&sql)
            .bind(n.tenant_id).bind(n.owner_user_id).bind(n.parent_id)
            .bind(n.name).bind(n.mime_type).bind(n.total_size).bind(n.storage_key)
            .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn get(&self, tenant_id: Uuid, id: Uuid) -> Result<Option<UploadSession>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "SELECT {COLS} FROM drive_uploads \
             WHERE id = $1 AND tenant_id = $2"
        );
        let row: Option<UploadSession> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }

    /// List storage keys for expired rows — callers use this to best-effort
    /// remove the matching `.part` blobs before (or after) invoking the SQL
    /// purge function. Cross-tenant: no RLS filter, only used by GC task.
    pub async fn list_expired_keys(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT storage_key FROM drive_uploads WHERE expires_at < now()"
        )
        .fetch_all(self.pool).await?;
        Ok(rows.into_iter().map(|(k,)| k).collect())
    }

    /// Invoke the server-side `drive_uploads_purge_expired()` function
    /// (batched 5000 per loop in SQL). Returns total rows deleted.
    pub async fn purge_expired(&self) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT drive_uploads_purge_expired()")
            .fetch_one(self.pool).await?;
        Ok(n)
    }

    /// Retorna novo offset se update bem-sucedido (usa compare-and-set para
    /// evitar gravações concorrentes fora de ordem).
    pub async fn advance_offset(
        &self,
        tenant_id:    Uuid,
        id:           Uuid,
        expected:     i64,
        new_offset:   i64,
    ) -> Result<Option<i64>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row: Option<(i64,)> = sqlx::query_as(
            "UPDATE drive_uploads \
                SET offset_bytes = $4 \
              WHERE id = $1 AND tenant_id = $2 AND offset_bytes = $3 \
              RETURNING offset_bytes"
        )
        .bind(id).bind(tenant_id).bind(expected).bind(new_offset)
        .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(row.map(|(o,)| o))
    }

    pub async fn delete(&self, tenant_id: Uuid, id: Uuid) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let r = sqlx::query("DELETE FROM drive_uploads WHERE id = $1 AND tenant_id = $2")
            .bind(id).bind(tenant_id)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(r.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    fn session_with_expiry(offset: Duration) -> UploadSession {
        UploadSession {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            owner_user_id: Uuid::nil(),
            parent_id: None,
            name: "x".into(),
            mime_type: None,
            total_size: 1,
            offset_bytes: 0,
            storage_key: "k".into(),
            created_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc() + offset,
        }
    }

    #[test]
    fn is_expired_past() {
        assert!(session_with_expiry(Duration::minutes(-1)).is_expired());
    }

    #[test]
    fn is_expired_future() {
        assert!(!session_with_expiry(Duration::hours(1)).is_expired());
    }
}

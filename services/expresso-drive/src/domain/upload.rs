//! Drive resumable uploads repository — tus.io server state.

use expresso_core::DbPool;
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
        let sql = format!(
            "INSERT INTO drive_uploads (tenant_id, owner_user_id, parent_id, name, \
                mime_type, total_size, storage_key) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) \
             RETURNING {COLS}"
        );
        let row: UploadSession = sqlx::query_as(&sql)
            .bind(n.tenant_id).bind(n.owner_user_id).bind(n.parent_id)
            .bind(n.name).bind(n.mime_type).bind(n.total_size).bind(n.storage_key)
            .fetch_one(self.pool).await?;
        Ok(row)
    }

    pub async fn get(&self, tenant_id: Uuid, id: Uuid) -> Result<Option<UploadSession>> {
        let sql = format!(
            "SELECT {COLS} FROM drive_uploads \
             WHERE id = $1 AND tenant_id = $2 AND expires_at > now()"
        );
        let row: Option<UploadSession> = sqlx::query_as(&sql)
            .bind(id).bind(tenant_id)
            .fetch_optional(self.pool).await?;
        Ok(row)
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
        let row: Option<(i64,)> = sqlx::query_as(
            "UPDATE drive_uploads \
                SET offset_bytes = $4 \
              WHERE id = $1 AND tenant_id = $2 AND offset_bytes = $3 \
              RETURNING offset_bytes"
        )
        .bind(id).bind(tenant_id).bind(expected).bind(new_offset)
        .fetch_optional(self.pool).await?;
        Ok(row.map(|(o,)| o))
    }

    pub async fn delete(&self, tenant_id: Uuid, id: Uuid) -> Result<u64> {
        let r = sqlx::query("DELETE FROM drive_uploads WHERE id = $1 AND tenant_id = $2")
            .bind(id).bind(tenant_id)
            .execute(self.pool).await?;
        Ok(r.rows_affected())
    }
}

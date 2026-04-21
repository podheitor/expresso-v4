//! Drive shared links — public download via token.
//!
//! Token entregue uma vez ao criador; apenas sha256(token) persistido.
//! Revogação por id; expiração por timestamp.

use expresso_core::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Share {
    pub id:         Uuid,
    pub tenant_id:  Uuid,
    pub file_id:    Uuid,
    pub permission: String,
    pub created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ResolvedShare {
    pub id:         Uuid,
    pub tenant_id:  Uuid,
    pub file_id:    Uuid,
    pub expires_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

pub struct ShareRepo<'a> {
    pool: &'a DbPool,
}

const SELECT_COLS: &str = "id, tenant_id, file_id, permission, created_by, \
    created_at, expires_at, revoked_at";

impl<'a> ShareRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    pub async fn insert(
        &self,
        tenant_id:  Uuid,
        file_id:    Uuid,
        token_hash: &str,
        created_by: Uuid,
        expires_at: OffsetDateTime,
    ) -> Result<Share> {
        let sql = format!(
            "INSERT INTO drive_shares (tenant_id, file_id, token_hash, created_by, expires_at) \
             VALUES ($1,$2,$3,$4,$5) \
             RETURNING {SELECT_COLS}"
        );
        let row = sqlx::query_as(&sql)
            .bind(tenant_id).bind(file_id).bind(token_hash)
            .bind(created_by).bind(expires_at)
            .fetch_one(self.pool).await?;
        Ok(row)
    }

    pub async fn list_for_file(&self, tenant_id: Uuid, file_id: Uuid) -> Result<Vec<Share>> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_shares \
             WHERE tenant_id = $1 AND file_id = $2 \
             ORDER BY created_at DESC"
        );
        let rows = sqlx::query_as(&sql)
            .bind(tenant_id).bind(file_id)
            .fetch_all(self.pool).await?;
        Ok(rows)
    }

    pub async fn revoke(&self, tenant_id: Uuid, id: Uuid) -> Result<u64> {
        let r = sqlx::query(
            "UPDATE drive_shares SET revoked_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND revoked_at IS NULL",
        )
        .bind(id).bind(tenant_id)
        .execute(self.pool).await?;
        Ok(r.rows_affected())
    }

    /// Resolve via SECURITY DEFINER fn — sem contexto de tenant.
    pub async fn resolve(&self, token_hash: &str) -> Result<Option<ResolvedShare>> {
        let row: Option<ResolvedShare> = sqlx::query_as(
            "SELECT id, tenant_id, file_id, expires_at, revoked_at \
             FROM drive_share_resolve($1)",
        )
        .bind(token_hash)
        .fetch_optional(self.pool).await?;
        Ok(row)
    }
}

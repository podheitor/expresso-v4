//! File tag repository — `drive_file_tags(tenant_id, file_id, tag)`.

use expresso_core::{begin_tenant_tx, DbPool};
use uuid::Uuid;

use crate::error::{DriveError, Result};

pub struct TagRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> TagRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    pub async fn list(&self, tenant_id: Uuid, file_id: Uuid) -> Result<Vec<String>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT tag FROM drive_file_tags \
             WHERE tenant_id = $1 AND file_id = $2 \
             ORDER BY tag",
        )
        .bind(tenant_id)
        .bind(file_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows.into_iter().map(|(t,)| t).collect())
    }

    pub async fn add(&self, tenant_id: Uuid, file_id: Uuid, tag: &str) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        sqlx::query(
            "INSERT INTO drive_file_tags (tenant_id, file_id, tag) \
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(tenant_id)
        .bind(file_id)
        .bind(tag)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn remove(&self, tenant_id: Uuid, file_id: Uuid, tag: &str) -> Result<bool> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let r = sqlx::query(
            "DELETE FROM drive_file_tags \
             WHERE tenant_id = $1 AND file_id = $2 AND tag = $3",
        )
        .bind(tenant_id)
        .bind(file_id)
        .bind(tag)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(r.rows_affected() > 0)
    }
}

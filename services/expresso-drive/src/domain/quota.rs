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

use crate::error::Result;

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

pub struct QuotaRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> QuotaRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

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

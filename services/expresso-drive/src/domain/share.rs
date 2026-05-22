//! Drive shared links — public download via token.
//!
//! Token entregue uma vez ao criador; apenas sha256(token) persistido.
//! Revogação por id; expiração por timestamp.
//!
//! Tenant scoping: cada método autenticado abre uma transação via
//! `begin_tenant_tx` para que as policies de RLS de `drive_shares`
//! filtrem por `current_setting('app.tenant_id')`. As cláusulas
//! `WHERE tenant_id = $1` permanecem como defense-in-depth — caso o
//! deployment use uma role com BYPASSRLS, o filtro explícito ainda
//! protege. `resolve` continua fora desse esquema porque o endpoint
//! público não tem contexto de tenant (usa SECURITY DEFINER fn).

use expresso_core::{begin_tenant_tx, DbPool};
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
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "INSERT INTO drive_shares (tenant_id, file_id, token_hash, created_by, expires_at) \
             VALUES ($1,$2,$3,$4,$5) \
             RETURNING {SELECT_COLS}"
        );
        let row = sqlx::query_as(&sql)
            .bind(tenant_id).bind(file_id).bind(token_hash)
            .bind(created_by).bind(expires_at)
            .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn list_for_file(&self, tenant_id: Uuid, file_id: Uuid) -> Result<Vec<Share>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_shares \
             WHERE tenant_id = $1 AND file_id = $2 \
             ORDER BY created_at DESC"
        );
        let rows = sqlx::query_as(&sql)
            .bind(tenant_id).bind(file_id)
            .fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn revoke(&self, tenant_id: Uuid, id: Uuid) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let r = sqlx::query(
            "UPDATE drive_shares SET revoked_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND revoked_at IS NULL",
        )
        .bind(id).bind(tenant_id)
        .execute(&mut *tx).await?;
        tx.commit().await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn share_serde_roundtrip() {
        let s = Share {
            id:         Uuid::nil(),
            tenant_id:  Uuid::nil(),
            file_id:    Uuid::nil(),
            permission: "read".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-05-22 09:00:00 UTC),
            expires_at: datetime!(2026-05-29 09:00:00 UTC),
            revoked_at: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Share = serde_json::from_str(&json).unwrap();
        assert_eq!(back.permission, "read");
        assert!(back.revoked_at.is_none());
    }

    #[test]
    fn share_revoked_at_present_roundtrip() {
        let s = Share {
            id: Uuid::nil(), tenant_id: Uuid::nil(), file_id: Uuid::nil(),
            permission: "write".into(), created_by: Uuid::nil(),
            created_at: datetime!(2026-05-22 09:00:00 UTC),
            expires_at: datetime!(2026-05-29 09:00:00 UTC),
            revoked_at: Some(datetime!(2026-05-23 10:00:00 UTC)),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Share = serde_json::from_str(&json).unwrap();
        assert!(back.revoked_at.is_some());
    }

    #[test]
    fn share_expires_at_in_rfc3339() {
        let s = Share {
            id: Uuid::nil(), tenant_id: Uuid::nil(), file_id: Uuid::nil(),
            permission: "read".into(), created_by: Uuid::nil(),
            created_at: datetime!(2026-05-22 09:00:00 UTC),
            expires_at: datetime!(2026-06-22 09:00:00 UTC),
            revoked_at: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("2026-06-22T09:00:00"));
    }

    #[test]
    fn share_debug_contains_permission() {
        let s = Share {
            id: Uuid::nil(), tenant_id: Uuid::nil(), file_id: Uuid::nil(),
            permission: "write".into(), created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert!(format!("{s:?}").contains("write"));
    }

    #[test]
    fn share_different_permissions_serde() {
        for perm in ["read", "write", "admin"] {
            let s = Share {
                id: Uuid::nil(), tenant_id: Uuid::nil(), file_id: Uuid::nil(),
                permission: perm.into(), created_by: Uuid::nil(),
                created_at: datetime!(2026-01-01 00:00:00 UTC),
                expires_at: datetime!(2026-02-01 00:00:00 UTC),
                revoked_at: None,
            };
            let back: Share = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
            assert_eq!(back.permission, perm);
        }
    }

    #[test]
    fn share_revoked_at_none_by_default() {
        let s = Share {
            id: Uuid::nil(), tenant_id: Uuid::nil(), file_id: Uuid::nil(),
            permission: "read".into(), created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert!(s.revoked_at.is_none());
    }

    #[test]
    fn share_file_id_accessible() {
        let fid = Uuid::new_v4();
        let s = Share {
            id: Uuid::nil(), tenant_id: Uuid::nil(), file_id: fid,
            permission: "read".into(), created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert_eq!(s.file_id, fid);
    }

    #[test]
    fn share_permission_preserved() {
        let s = Share {
            id: Uuid::nil(), tenant_id: Uuid::nil(), file_id: Uuid::nil(),
            permission: "write".into(), created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert_eq!(s.permission, "write");
    }

    #[test]
    fn share_expires_at_accessible() {
        let s = Share {
            id: Uuid::nil(), tenant_id: Uuid::nil(), file_id: Uuid::nil(),
            permission: "read".into(), created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-06-30 00:00:00 UTC),
            revoked_at: None,
        };
        assert!(s.expires_at > s.created_at);
    }

    #[test]
    fn share_permission_read_preserved() {
        let s = Share {
            id: Uuid::nil(), tenant_id: Uuid::nil(), file_id: Uuid::nil(),
            permission: "read".into(), created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert_eq!(s.permission, "read");
    }
}

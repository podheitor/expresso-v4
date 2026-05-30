//! Drive file/folder ACLs — per-user sharing (mirror of calendar/addressbook ACL).
//!
//! `access_level` resolves a user's effective privilege on a node: the owner is
//! `OWNER` (implicit ADMIN), a grantee gets their `drive_file_acl.privilege`,
//! and everyone else is `None`. Grants work for files and folders alike; a
//! folder grant is intended to cover descendants (enforced in handlers, not SQL).

use expresso_core::{begin_tenant_tx, DbPool};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{DriveError, Result};

/// Privileges a grant may carry. `OWNER` is synthetic (never stored) — it is
/// what `access_level` returns for the node's owner.
pub const PRIV_READ: &str = "READ";
pub const PRIV_WRITE: &str = "WRITE";
pub const PRIV_ADMIN: &str = "ADMIN";

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FileAcl {
    pub file_id: Uuid,
    pub tenant_id: Uuid,
    pub grantee_id: Uuid,
    pub privilege: String,
}

#[derive(Clone)]
pub struct AclRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> AclRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Resolve `user_id`'s effective privilege on `file_id`, honoring folder
    /// inheritance: `Some("OWNER")` for the node owner, otherwise the strongest
    /// grant on the node OR any ancestor folder (a grant on a folder cascades
    /// to its descendants), or `None` for no access / a missing-or-trashed node.
    ///
    /// The ancestor walk is a single recursive CTE over `parent_id`; the
    /// strongest of the matched privileges wins (ADMIN > WRITE > READ).
    pub async fn access_level(
        &self,
        tenant_id: Uuid,
        file_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<String>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let owner: Option<(Uuid,)> = sqlx::query_as(
            "SELECT owner_user_id FROM drive_files \
             WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL",
        )
        .bind(file_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await?;
        match owner {
            None => {
                tx.commit().await?;
                return Ok(None);
            }
            Some((o,)) if o == user_id => {
                tx.commit().await?;
                return Ok(Some("OWNER".into()));
            }
            _ => {}
        }
        // Walk the node + its ancestors; take the strongest grant for the user.
        let rank: Option<(i32,)> = sqlx::query_as(
            "WITH RECURSIVE chain AS ( \
                 SELECT id, parent_id FROM drive_files \
                  WHERE id = $1 AND tenant_id = $2 \
                 UNION ALL \
                 SELECT f.id, f.parent_id FROM drive_files f \
                  JOIN chain c ON f.id = c.parent_id \
                  WHERE f.tenant_id = $2 \
             ) \
             SELECT MAX(CASE a.privilege \
                          WHEN 'ADMIN' THEN 3 WHEN 'WRITE' THEN 2 WHEN 'READ' THEN 1 \
                          ELSE 0 END) \
               FROM drive_file_acl a \
               JOIN chain c ON c.id = a.file_id \
              WHERE a.tenant_id = $2 AND a.grantee_id = $3",
        )
        .bind(file_id)
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rank.and_then(|(r,)| match r {
            3 => Some(PRIV_ADMIN.to_string()),
            2 => Some(PRIV_WRITE.to_string()),
            1 => Some(PRIV_READ.to_string()),
            _ => None,
        }))
    }

    /// Upsert a grant. Idempotent: re-granting a different privilege updates it.
    pub async fn grant(
        &self,
        tenant_id: Uuid,
        file_id: Uuid,
        grantee_id: Uuid,
        privilege: &str,
    ) -> Result<FileAcl> {
        if !matches!(privilege, PRIV_READ | PRIV_WRITE | PRIV_ADMIN) {
            return Err(DriveError::BadRequest(format!(
                "invalid privilege: {privilege}"
            )));
        }
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, FileAcl>(
            "INSERT INTO drive_file_acl (file_id, tenant_id, grantee_id, privilege) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (file_id, grantee_id) DO UPDATE SET privilege = EXCLUDED.privilege \
             RETURNING file_id, tenant_id, grantee_id, privilege",
        )
        .bind(file_id)
        .bind(tenant_id)
        .bind(grantee_id)
        .bind(privilege)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Remove a grant. `NotFound` when the pair had none, so a handler can 404.
    pub async fn revoke(&self, tenant_id: Uuid, file_id: Uuid, grantee_id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let res = sqlx::query(
            "DELETE FROM drive_file_acl \
             WHERE file_id = $1 AND tenant_id = $2 AND grantee_id = $3",
        )
        .bind(file_id)
        .bind(tenant_id)
        .bind(grantee_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        if res.rows_affected() == 0 {
            return Err(DriveError::NotFound(file_id));
        }
        Ok(())
    }

    /// List the grants on a node, newest first.
    pub async fn list(&self, tenant_id: Uuid, file_id: Uuid) -> Result<Vec<FileAcl>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query_as::<_, FileAcl>(
            "SELECT file_id, tenant_id, grantee_id, privilege FROM drive_file_acl \
             WHERE file_id = $1 AND tenant_id = $2 ORDER BY created_at DESC",
        )
        .bind(file_id)
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privilege_constants() {
        assert_eq!(PRIV_READ, "READ");
        assert_eq!(PRIV_WRITE, "WRITE");
        assert_eq!(PRIV_ADMIN, "ADMIN");
    }

    #[test]
    fn file_acl_serde_roundtrip() {
        let a = FileAcl {
            file_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            grantee_id: Uuid::nil(),
            privilege: "READ".into(),
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: FileAcl = serde_json::from_str(&s).unwrap();
        assert_eq!(back.privilege, "READ");
    }
}

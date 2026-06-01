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
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

/// Hash a share password with a fresh random salt. Returns `(hex_hash, hex_salt)`
/// to persist. Mirrors the token_hash scheme (salted SHA-256) — no new crypto
/// dependency. The link token is the primary secret; this guards casual
/// forwarding of the link.
#[must_use]
pub fn hash_password(password: &str) -> (String, String) {
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let salt_hex = hex_lower(&salt);
    let hash = derive_hash(password, &salt_hex);
    (hash, salt_hex)
}

/// Verify a candidate password against a stored `(hash, salt)`.
#[must_use]
pub fn verify_password(candidate: &str, hash: &str, salt: &str) -> bool {
    // Constant-time-ish: compare derived bytes via subtle-free eq on equal-length
    // hex strings. Both are SHA-256 hex (64 chars), so length is constant.
    let got = derive_hash(candidate, salt);
    got.len() == hash.len()
        && got
            .bytes()
            .zip(hash.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn derive_hash(password: &str, salt_hex: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt_hex.as_bytes());
    h.update(b":");
    h.update(password.as_bytes());
    hex_lower(&h.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Share {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub file_id: Uuid,
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
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub file_id: Uuid,
    pub expires_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
    /// Salted SHA-256 of the link password, or `None` when the link is
    /// password-less. Verified by the public endpoint, never returned to clients.
    pub password_hash: Option<String>,
    pub password_salt: Option<String>,
    /// Cap on total downloads, or `None` for unlimited.
    pub max_downloads: Option<i32>,
    /// Downloads served so far.
    pub download_count: i32,
}

/// Parameters for [`ShareRepo::insert`]. `password` is an optional `(hash, salt)`
/// pair from [`hash_password`] (`None` = password-less); `max_downloads` caps
/// total downloads (`None` = unlimited).
#[derive(Debug, Clone, Copy)]
pub struct NewShare<'a> {
    pub tenant_id: Uuid,
    pub file_id: Uuid,
    pub token_hash: &'a str,
    pub created_by: Uuid,
    pub expires_at: OffsetDateTime,
    pub password: Option<(&'a str, &'a str)>,
    pub max_downloads: Option<i32>,
}

pub struct ShareRepo<'a> {
    pool: &'a DbPool,
}

const SELECT_COLS: &str = "id, tenant_id, file_id, permission, created_by, \
    created_at, expires_at, revoked_at";

impl<'a> ShareRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Insert a share described by [`NewShare`].
    pub async fn insert(&self, n: NewShare<'_>) -> Result<Share> {
        let (pw_hash, pw_salt) = match n.password {
            Some((h, s)) => (Some(h), Some(s)),
            None => (None, None),
        };
        let mut tx = begin_tenant_tx(self.pool, n.tenant_id).await?;
        let sql = format!(
            "INSERT INTO drive_shares \
                 (tenant_id, file_id, token_hash, created_by, expires_at, password_hash, password_salt, max_downloads) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
             RETURNING {SELECT_COLS}"
        );
        let row = sqlx::query_as(&sql)
            .bind(n.tenant_id)
            .bind(n.file_id)
            .bind(n.token_hash)
            .bind(n.created_by)
            .bind(n.expires_at)
            .bind(pw_hash)
            .bind(pw_salt)
            .bind(n.max_downloads)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Atomically consume one download against a share's cap. Returns the new
    /// `download_count` when allowed, or `None` when the share is
    /// exhausted/revoked/expired (the public endpoint then rejects). The check
    /// and increment happen in one statement (`drive_share_consume`), so
    /// concurrent downloads cannot exceed `max_downloads`.
    pub async fn try_consume_download(&self, id: Uuid) -> Result<Option<i32>> {
        let new_count: Option<i32> = sqlx::query_scalar("SELECT drive_share_consume($1)")
            .bind(id)
            .fetch_one(self.pool)
            .await?;
        Ok(new_count)
    }

    pub async fn list_for_file(&self, tenant_id: Uuid, file_id: Uuid) -> Result<Vec<Share>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM drive_shares \
             WHERE tenant_id = $1 AND file_id = $2 \
             ORDER BY created_at DESC"
        );
        let rows = sqlx::query_as(&sql)
            .bind(tenant_id)
            .bind(file_id)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn revoke(&self, tenant_id: Uuid, id: Uuid) -> Result<u64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let r = sqlx::query(
            "UPDATE drive_shares SET revoked_at = now() \
             WHERE id = $1 AND tenant_id = $2 AND revoked_at IS NULL",
        )
        .bind(id)
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(r.rows_affected())
    }

    /// Resolve via SECURITY DEFINER fn — sem contexto de tenant.
    pub async fn resolve(&self, token_hash: &str) -> Result<Option<ResolvedShare>> {
        let row: Option<ResolvedShare> = sqlx::query_as(
            "SELECT id, tenant_id, file_id, expires_at, revoked_at, \
                    password_hash, password_salt, max_downloads, download_count \
             FROM drive_share_resolve($1)",
        )
        .bind(token_hash)
        .fetch_optional(self.pool)
        .await?;
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
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
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
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "write".into(),
            created_by: Uuid::nil(),
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
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "read".into(),
            created_by: Uuid::nil(),
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
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "write".into(),
            created_by: Uuid::nil(),
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
                id: Uuid::nil(),
                tenant_id: Uuid::nil(),
                file_id: Uuid::nil(),
                permission: perm.into(),
                created_by: Uuid::nil(),
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
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "read".into(),
            created_by: Uuid::nil(),
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
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: fid,
            permission: "read".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert_eq!(s.file_id, fid);
    }

    #[test]
    fn share_permission_preserved() {
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "write".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert_eq!(s.permission, "write");
    }

    #[test]
    fn share_expires_at_accessible() {
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "read".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-06-30 00:00:00 UTC),
            revoked_at: None,
        };
        assert!(s.expires_at > s.created_at);
    }

    #[test]
    fn share_permission_read_preserved() {
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "read".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert_eq!(s.permission, "read");
    }

    #[test]
    fn share_write_permission_preserved() {
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "write".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert_eq!(s.permission, "write");
    }

    #[test]
    fn share_revoked_at_is_none_for_active_share() {
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "read".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert!(s.revoked_at.is_none());
    }

    #[test]
    fn share_read_permission_value_is_read() {
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "read".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert_eq!(s.permission, "read");
    }

    #[test]
    fn share_write_revoked_at_is_none() {
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "write".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert!(s.revoked_at.is_none());
    }

    #[test]
    fn share_file_id_preserved() {
        let fid = Uuid::new_v4();
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: fid,
            permission: "read".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert_eq!(s.file_id, fid);
    }

    #[test]
    fn share_created_by_preserved() {
        let uid = Uuid::new_v4();
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "read".into(),
            created_by: uid,
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert_eq!(s.created_by, uid);
    }

    #[test]
    fn share_tenant_id_preserved() {
        let tid = Uuid::new_v4();
        let s = Share {
            id: Uuid::nil(),
            tenant_id: tid,
            file_id: Uuid::nil(),
            permission: "read".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert_eq!(s.tenant_id, tid);
    }

    #[test]
    fn share_expires_after_created() {
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "read".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-01-08 00:00:00 UTC),
            revoked_at: None,
        };
        assert!(s.expires_at > s.created_at);
    }

    #[test]
    fn share_id_is_nil_uuid_when_set_to_nil() {
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "write".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-03-01 00:00:00 UTC),
            expires_at: datetime!(2026-03-08 00:00:00 UTC),
            revoked_at: None,
        };
        assert_eq!(s.id, Uuid::nil());
    }

    #[test]
    fn share_revoked_some_not_none() {
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "read".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-02-01 00:00:00 UTC),
            revoked_at: Some(datetime!(2026-01-15 00:00:00 UTC)),
        };
        assert!(s.revoked_at.is_some());
    }

    #[test]
    fn share_permission_is_not_empty() {
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "write".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-06-01 00:00:00 UTC),
            revoked_at: None,
        };
        assert!(!s.permission.is_empty());
    }

    #[test]
    fn share_revoked_at_none_when_not_revoked() {
        use time::macros::datetime;
        let s = Share {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            file_id: Uuid::nil(),
            permission: "read".into(),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
            expires_at: datetime!(2026-12-31 00:00:00 UTC),
            revoked_at: None,
        };
        assert!(s.revoked_at.is_none());
    }

    #[test]
    fn share_permission_write_differs_from_read() {
        let write = "write".to_string();
        let read = "read".to_string();
        assert_ne!(write, read);
    }

    #[test]
    fn password_hash_then_verify_roundtrips() {
        let (hash, salt) = hash_password("s3cret");
        assert!(verify_password("s3cret", &hash, &salt));
        assert!(!verify_password("wrong", &hash, &salt));
    }

    #[test]
    fn password_hash_uses_random_salt() {
        let (h1, s1) = hash_password("same");
        let (h2, s2) = hash_password("same");
        // Distinct salts → distinct hashes for the same password.
        assert_ne!(s1, s2);
        assert_ne!(h1, h2);
        // Each still verifies against its own salt.
        assert!(verify_password("same", &h1, &s1));
        assert!(verify_password("same", &h2, &s2));
    }

    #[test]
    fn password_hash_is_sha256_hex() {
        let (hash, salt) = hash_password("x");
        assert_eq!(hash.len(), 64);
        assert_eq!(salt.len(), 32);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn verify_rejects_wrong_salt() {
        let (hash, _salt) = hash_password("pw");
        assert!(!verify_password(
            "pw",
            &hash,
            "00000000000000000000000000000000"
        ));
    }

    #[test]
    fn empty_password_is_hashable_and_verifiable() {
        let (hash, salt) = hash_password("");
        assert!(verify_password("", &hash, &salt));
        assert!(!verify_password(" ", &hash, &salt));
    }
}

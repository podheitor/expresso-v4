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

    /// Delete a specific version row. Returns the deleted row so the caller can
    /// remove the blob from storage. Returns None if not found.
    pub async fn delete(&self, tenant_id: Uuid, file_id: Uuid, version_no: i32) -> Result<Option<FileVersion>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let sql = format!(
            "DELETE FROM drive_file_versions \
             WHERE tenant_id = $1 AND file_id = $2 AND version_no = $3 \
             RETURNING {SELECT_COLS}"
        );
        let row = sqlx::query_as(&sql)
            .bind(tenant_id).bind(file_id).bind(version_no)
            .fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn file_version_serde_roundtrip() {
        let v = FileVersion {
            id:          Uuid::nil(),
            file_id:     Uuid::nil(),
            tenant_id:   Uuid::nil(),
            version_no:  3,
            storage_key: "blobs/abc".into(),
            size_bytes:  4096,
            sha256:      Some("abcdef".into()),
            mime_type:   Some("application/pdf".into()),
            created_by:  Uuid::nil(),
            created_at:  datetime!(2026-05-22 10:00:00 UTC),
        };
        let s = serde_json::to_string(&v).unwrap();
        let back: FileVersion = serde_json::from_str(&s).unwrap();
        assert_eq!(back.version_no, 3);
        assert_eq!(back.size_bytes, 4096);
        assert_eq!(back.sha256.as_deref(), Some("abcdef"));
    }

    #[test]
    fn file_version_created_at_rfc3339() {
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_no: 1, storage_key: "k".into(), size_bytes: 0,
            sha256: None, mime_type: None, created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains("2026-01-01T00:00:00"));
    }

    #[test]
    fn file_version_no_sha256_serialises_null() {
        use time::macros::datetime;
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_no: 2, storage_key: "k2".into(), size_bytes: 100,
            sha256: None, mime_type: None, created_by: Uuid::nil(),
            created_at: datetime!(2026-02-01 00:00:00 UTC),
        };
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains("\"sha256\":null"));
    }

    #[test]
    fn file_version_clone_preserves_storage_key() {
        use time::macros::datetime;
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_no: 5, storage_key: "blobs/v5/data".into(), size_bytes: 8192,
            sha256: Some("deadbeef".into()), mime_type: Some("text/plain".into()),
            created_by: Uuid::nil(),
            created_at: datetime!(2026-03-01 09:00:00 UTC),
        };
        let c = v.clone();
        assert_eq!(c.storage_key, "blobs/v5/data");
        assert_eq!(c.version_no, 5);
    }

    #[test]
    fn file_version_sha256_some_in_json() {
        use time::macros::datetime;
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_no: 1, storage_key: "k".into(), size_bytes: 1,
            sha256: Some("abc123".into()), mime_type: None,
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        let j: serde_json::Value = serde_json::to_value(&v).unwrap();
        assert_eq!(j["sha256"], "abc123");
    }

    #[test]
    fn file_version_size_bytes_zero_allowed() {
        use time::macros::datetime;
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_no: 1, storage_key: "k".into(), size_bytes: 0,
            sha256: None, mime_type: None,
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert_eq!(v.size_bytes, 0);
    }

    #[test]
    fn file_version_mime_type_none_serializes_null() {
        use time::macros::datetime;
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_no: 1, storage_key: "k".into(), size_bytes: 1,
            sha256: None, mime_type: None,
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        let j: serde_json::Value = serde_json::to_value(&v).unwrap();
        assert!(j["mime_type"].is_null());
    }

    #[test]
    fn file_version_storage_key_preserved() {
        use time::macros::datetime;
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_no: 7, storage_key: "blobs/tenant/v7".into(), size_bytes: 512,
            sha256: None, mime_type: None,
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert_eq!(v.storage_key, "blobs/tenant/v7");
        assert_eq!(v.version_no, 7);
    }

    #[test]
    fn file_version_sha256_none_accessible() {
        use time::macros::datetime;
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_no: 1, storage_key: "blobs/v1".into(), size_bytes: 256,
            sha256: None, mime_type: None,
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert!(v.sha256.is_none());
    }

    #[test]
    fn file_version_size_bytes_accessible() {
        use time::macros::datetime;
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_no: 2, storage_key: "blobs/v2".into(), size_bytes: 4096,
            sha256: None, mime_type: None,
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert_eq!(v.size_bytes, 4096);
    }

    #[test]
    fn file_version_storage_key_roundtrip() {
        use time::macros::datetime;
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_no: 3, storage_key: "blobs/v3".into(), size_bytes: 0,
            sha256: None, mime_type: None,
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert_eq!(v.storage_key, "blobs/v3");
    }

    #[test]
    fn file_version_version_no_preserved() {
        use time::macros::datetime;
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_no: 7, storage_key: "blobs/v7".into(), size_bytes: 1024,
            sha256: None, mime_type: None,
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert_eq!(v.version_no, 7);
    }

    #[test]
    fn file_version_size_bytes_preserved() {
        use time::macros::datetime;
        let v = FileVersion {
            id: Uuid::nil(), file_id: Uuid::nil(), tenant_id: Uuid::nil(),
            version_no: 1, storage_key: "blobs/v1".into(), size_bytes: 2048,
            sha256: None, mime_type: None,
            created_by: Uuid::nil(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert_eq!(v.size_bytes, 2048);
    }
}

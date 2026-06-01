//! Contact version history domain + repository.
//!
//! Each REST update snapshots the contact's prior vCard into `contact_versions`.
//! A contact is stored as an opaque `vcard_raw` blob, so a snapshot captures the
//! whole raw (the source of truth) rather than per-field — restore re-applies it
//! via [`ContactRepo::update`](super::contact::ContactRepo::update), which itself
//! snapshots, so a restore is also versioned (an undo is reversible). `full_name`
//! is denormalised alongside for display in the version list without re-parsing.
//! `version_no` is assigned per contact as `max + 1` inside the snapshot
//! transaction, so concurrent edits get distinct numbers. Tenant scoping via
//! `begin_tenant_tx` + explicit `WHERE`.

use expresso_core::{begin_tenant_tx, DbPool};
use serde::Serialize;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{ContactsError, Result};

/// A recorded prior revision of a contact's vCard.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ContactVersion {
    pub id: Uuid,
    pub contact_id: Uuid,
    pub tenant_id: Uuid,
    pub version_no: i32,
    pub vcard_raw: String,
    pub full_name: Option<String>,
    pub edited_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

pub struct ContactVersionRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> ContactVersionRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Snapshot a contact's prior vCard as the next version. `edited_by` is the
    /// user making the edit that supersedes this content.
    pub async fn snapshot(
        &self,
        tenant: Uuid,
        contact_id: Uuid,
        vcard_raw: &str,
        full_name: Option<&str>,
        edited_by: Uuid,
    ) -> Result<ContactVersion> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, ContactVersion>(
            r#"INSERT INTO contact_versions
                   (contact_id, tenant_id, version_no, vcard_raw, full_name, edited_by)
               SELECT $1, $2,
                      COALESCE(MAX(version_no), 0) + 1, $3, $4, $5
                 FROM contact_versions WHERE contact_id = $1 AND tenant_id = $2
               RETURNING *"#,
        )
        .bind(contact_id)
        .bind(tenant)
        .bind(vcard_raw)
        .bind(full_name)
        .bind(edited_by)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// List a contact's versions, newest first. The full `vcard_raw` of each
    /// snapshot is included so a client can diff/preview without a second call.
    pub async fn list(&self, tenant: Uuid, contact_id: Uuid) -> Result<Vec<ContactVersion>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows = sqlx::query_as::<_, ContactVersion>(
            r#"SELECT * FROM contact_versions
                WHERE tenant_id = $1 AND contact_id = $2
                ORDER BY version_no DESC"#,
        )
        .bind(tenant)
        .bind(contact_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Fetch one version by its number within a contact. 404 if absent.
    pub async fn get(
        &self,
        tenant: Uuid,
        contact_id: Uuid,
        version_no: i32,
    ) -> Result<ContactVersion> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row = sqlx::query_as::<_, ContactVersion>(
            r#"SELECT * FROM contact_versions
                WHERE tenant_id = $1 AND contact_id = $2 AND version_no = $3"#,
        )
        .bind(tenant)
        .bind(contact_id)
        .bind(version_no)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ContactsError::VersionNotFound(version_no))?;
        tx.commit().await?;
        Ok(row)
    }
}

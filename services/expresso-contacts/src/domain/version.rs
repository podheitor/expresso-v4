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

/// A line-level diff between two vCard snapshots. vCard is line-oriented (one
/// property per logical line), so removed/added lines correspond to changed
/// properties. Unchanged lines are omitted. This is a content diff, not a
/// positional one: a line present in both is "unchanged" regardless of order,
/// which suits property sets where order is not semantically meaningful.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct VCardDiff {
    /// Lines in the newer version that were not in the older one.
    pub added: Vec<String>,
    /// Lines in the older version that are gone in the newer one.
    pub removed: Vec<String>,
}

/// Compute the line-level content diff from `old` to `new`. Blank lines are
/// ignored. Duplicate identical lines collapse (a property set, not a multiset).
pub fn diff_vcards(old: &str, new: &str) -> VCardDiff {
    use std::collections::BTreeSet;
    let lines = |s: &str| -> BTreeSet<String> {
        s.lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let old_set = lines(old);
    let new_set = lines(new);
    VCardDiff {
        added: new_set.difference(&old_set).cloned().collect(),
        removed: old_set.difference(&new_set).cloned().collect(),
    }
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

#[cfg(test)]
mod tests {
    use super::diff_vcards;

    #[test]
    fn diff_detects_changed_property() {
        let old = "BEGIN:VCARD\nFN:Alice\nTEL:111\nEND:VCARD";
        let new = "BEGIN:VCARD\nFN:Alice\nTEL:222\nEND:VCARD";
        let d = diff_vcards(old, new);
        assert_eq!(d.added, vec!["TEL:222".to_string()]);
        assert_eq!(d.removed, vec!["TEL:111".to_string()]);
    }

    #[test]
    fn diff_detects_added_and_removed() {
        let old = "FN:Alice\nORG:Acme";
        let new = "FN:Alice\nEMAIL:a@x.com";
        let d = diff_vcards(old, new);
        assert_eq!(d.added, vec!["EMAIL:a@x.com".to_string()]);
        assert_eq!(d.removed, vec!["ORG:Acme".to_string()]);
    }

    #[test]
    fn diff_identical_is_empty() {
        let s = "BEGIN:VCARD\nFN:Bob\nEND:VCARD";
        let d = diff_vcards(s, s);
        assert!(d.added.is_empty() && d.removed.is_empty());
    }

    #[test]
    fn diff_ignores_blank_lines_and_trailing_ws() {
        let old = "FN:Bob\n\nTEL:1   ";
        let new = "FN:Bob\nTEL:1";
        let d = diff_vcards(old, new);
        assert!(d.added.is_empty() && d.removed.is_empty());
    }
}

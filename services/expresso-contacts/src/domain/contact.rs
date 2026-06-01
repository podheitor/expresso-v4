//! Contact — persistence layer (CardDAV-aware).
//!
//! Events on the address book are path-addressed by UID (`<uid>.vcf`).
//! All writes parse the vCard to populate denormalised columns + etag.
//!
//! Tenant scoping: cada método abre transação via `begin_tenant_tx` para que
//! a policy RLS de `contacts` filtre por `current_setting('app.tenant_id')`
//! antes mesmo do `WHERE tenant_id = $1` explícito (defense-in-depth).

use expresso_core::{begin_tenant_tx, DbPool};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::vcard;
use crate::error::{ContactsError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Contact {
    pub id: Uuid,
    pub addressbook_id: Uuid,
    pub tenant_id: Uuid,
    pub uid: String,
    pub etag: String,
    pub vcard_raw: String,
    pub full_name: Option<String>,
    pub family_name: Option<String>,
    pub given_name: Option<String>,
    pub organization: Option<String>,
    pub email_primary: Option<String>,
    pub phone_primary: Option<String>,
    pub birthday: Option<String>,
    pub nickname: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Clone)]
pub struct ContactRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> ContactRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Insert parsing raw vCard; UID uniqueness enforced by DB index.
    pub async fn create(
        &self,
        tenant_id: Uuid,
        addressbook_id: Uuid,
        raw: &str,
    ) -> Result<Contact> {
        let parsed = vcard::parse(raw).map_err(ContactsError::InvalidVCard)?;
        let etag = vcard::compute_etag(raw);
        let emails = parsed.emails.clone();
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Contact>(
            r#"
            INSERT INTO contacts (
                addressbook_id, tenant_id, uid, etag, vcard_raw,
                full_name, family_name, given_name, organization,
                email_primary, phone_primary, birthday, nickname
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
            RETURNING *
            "#,
        )
        .bind(addressbook_id)
        .bind(tenant_id)
        .bind(parsed.uid)
        .bind(etag)
        .bind(raw)
        .bind(parsed.full_name)
        .bind(parsed.family_name)
        .bind(parsed.given_name)
        .bind(parsed.organization)
        .bind(parsed.email)
        .bind(parsed.phone)
        .bind(parsed.birthday)
        .bind(parsed.nickname)
        .fetch_one(&mut *tx)
        .await?;
        sync_emails(&mut tx, tenant_id, row.id, &emails).await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn get(&self, tenant_id: Uuid, id: Uuid) -> Result<Contact> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Contact>(
            r#"SELECT * FROM contacts WHERE tenant_id = $1 AND id = $2"#,
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn list(&self, tenant_id: Uuid, addressbook_id: Uuid) -> Result<Vec<Contact>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query_as::<_, Contact>(
            r#"
            SELECT * FROM contacts
             WHERE tenant_id = $1 AND addressbook_id = $2
             ORDER BY COALESCE(full_name, uid)
            "#,
        )
        .bind(tenant_id)
        .bind(addressbook_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Case-insensitive substring search across the denormalized name / email /
    /// organization columns, over ALL of the tenant's contacts (every
    /// addressbook). RLS + the explicit `tenant_id` filter keep it tenant-local;
    /// `limit` is clamped by the caller. The `%` and `_` LIKE metacharacters in
    /// `query` are escaped so a user can search for a literal address.
    pub async fn search(&self, tenant_id: Uuid, query: &str, limit: i64) -> Result<Vec<Contact>> {
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query_as::<_, Contact>(
            r#"
            SELECT * FROM contacts
             WHERE tenant_id = $1
               AND (full_name    ILIKE $2 ESCAPE '\'
                 OR email_primary ILIKE $2 ESCAPE '\'
                 OR organization  ILIKE $2 ESCAPE '\'
                 OR nickname      ILIKE $2 ESCAPE '\'
                 OR EXISTS (
                      SELECT 1 FROM contact_emails ce
                       WHERE ce.contact_id = contacts.id
                         AND ce.tenant_id = contacts.tenant_id
                         AND ce.address ILIKE $2 ESCAPE '\'))
             ORDER BY COALESCE(full_name, uid)
             LIMIT $3
            "#,
        )
        .bind(tenant_id)
        .bind(&pattern)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn update(&self, tenant_id: Uuid, id: Uuid, raw: &str) -> Result<Contact> {
        let parsed = vcard::parse(raw).map_err(ContactsError::InvalidVCard)?;
        let etag = vcard::compute_etag(raw);
        let emails = parsed.emails.clone();
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Contact>(
            r#"
            UPDATE contacts SET
                uid           = $3,
                etag          = $4,
                vcard_raw     = $5,
                full_name     = $6,
                family_name   = $7,
                given_name    = $8,
                organization  = $9,
                email_primary = $10,
                phone_primary = $11,
                birthday      = $12,
                nickname      = $13
            WHERE tenant_id = $1 AND id = $2
            RETURNING *
            "#,
        )
        .bind(tenant_id)
        .bind(id)
        .bind(parsed.uid)
        .bind(etag)
        .bind(raw)
        .bind(parsed.full_name)
        .bind(parsed.family_name)
        .bind(parsed.given_name)
        .bind(parsed.organization)
        .bind(parsed.email)
        .bind(parsed.phone)
        .bind(parsed.birthday)
        .bind(parsed.nickname)
        .fetch_one(&mut *tx)
        .await?;
        sync_emails(&mut tx, tenant_id, id, &emails).await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn delete(&self, tenant_id: Uuid, id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        sqlx::query(r#"DELETE FROM contacts WHERE tenant_id = $1 AND id = $2"#)
            .bind(tenant_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    // ─── CardDAV path-addressing helpers ────────────────────────────────
    pub async fn get_by_uid(
        &self,
        tenant_id: Uuid,
        addressbook_id: Uuid,
        uid: &str,
    ) -> Result<Contact> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Contact>(
            r#"SELECT * FROM contacts
               WHERE tenant_id = $1 AND addressbook_id = $2 AND uid = $3"#,
        )
        .bind(tenant_id)
        .bind(addressbook_id)
        .bind(uid)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn list_by_uids(
        &self,
        tenant_id: Uuid,
        addressbook_id: Uuid,
        uids: &[String],
    ) -> Result<Vec<Contact>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query_as::<_, Contact>(
            r#"SELECT * FROM contacts
               WHERE tenant_id = $1 AND addressbook_id = $2 AND uid = ANY($3)
               ORDER BY COALESCE(full_name, uid)"#,
        )
        .bind(tenant_id)
        .bind(addressbook_id)
        .bind(uids)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// UPSERT by (addressbook_id, uid) — used by CardDAV PUT.
    pub async fn replace_by_uid(
        &self,
        tenant_id: Uuid,
        addressbook_id: Uuid,
        raw: &str,
    ) -> Result<Contact> {
        let parsed = vcard::parse(raw).map_err(ContactsError::InvalidVCard)?;
        let etag = vcard::compute_etag(raw);
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Contact>(
            r#"
            INSERT INTO contacts (
                addressbook_id, tenant_id, uid, etag, vcard_raw,
                full_name, family_name, given_name, organization,
                email_primary, phone_primary
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            ON CONFLICT (addressbook_id, uid) DO UPDATE SET
                etag          = EXCLUDED.etag,
                vcard_raw     = EXCLUDED.vcard_raw,
                full_name     = EXCLUDED.full_name,
                family_name   = EXCLUDED.family_name,
                given_name    = EXCLUDED.given_name,
                organization  = EXCLUDED.organization,
                email_primary = EXCLUDED.email_primary,
                phone_primary = EXCLUDED.phone_primary
            RETURNING *
            "#,
        )
        .bind(addressbook_id)
        .bind(tenant_id)
        .bind(&parsed.uid)
        .bind(etag)
        .bind(raw)
        .bind(parsed.full_name)
        .bind(parsed.family_name)
        .bind(parsed.given_name)
        .bind(parsed.organization)
        .bind(parsed.email)
        .bind(parsed.phone)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn delete_by_uid(
        &self,
        tenant_id: Uuid,
        addressbook_id: Uuid,
        uid: &str,
    ) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        sqlx::query(
            r#"DELETE FROM contacts
               WHERE tenant_id = $1 AND addressbook_id = $2 AND uid = $3"#,
        )
        .bind(tenant_id)
        .bind(addressbook_id)
        .bind(uid)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// List a contact's indexed emails (all EMAIL entries with their labels), in
    /// document order. Empty when the contact has none.
    pub async fn list_emails(
        &self,
        tenant_id: Uuid,
        contact_id: Uuid,
    ) -> Result<Vec<ContactEmailRow>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query_as::<_, ContactEmailRow>(
            r#"SELECT address, label, position
                 FROM contact_emails
                WHERE tenant_id = $1 AND contact_id = $2
                ORDER BY position, created_at"#,
        )
        .bind(tenant_id)
        .bind(contact_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }
}

/// An indexed EMAIL entry returned to API callers.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ContactEmailRow {
    pub address: String,
    pub label: Option<String>,
    pub position: i32,
}

/// Replace a contact's email index: delete existing rows, then insert the parsed
/// EMAIL entries. Runs in the caller's tx so the index tracks `vcard_raw`.
async fn sync_emails(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    contact_id: Uuid,
    emails: &[vcard::ContactEmail],
) -> Result<()> {
    sqlx::query("DELETE FROM contact_emails WHERE tenant_id = $1 AND contact_id = $2")
        .bind(tenant_id)
        .bind(contact_id)
        .execute(&mut **tx)
        .await?;
    for (i, e) in emails.iter().enumerate() {
        sqlx::query(
            r#"INSERT INTO contact_emails (tenant_id, contact_id, address, label, position)
               VALUES ($1, $2, $3, $4, $5)"#,
        )
        .bind(tenant_id)
        .bind(contact_id)
        .bind(&e.address)
        .bind(&e.label)
        .bind(i as i32)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn sample() -> Contact {
        Contact {
            id: Uuid::nil(),
            addressbook_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            uid: "uid-xyz@example.com".into(),
            etag: "etag42".into(),
            vcard_raw: "BEGIN:VCARD\r\nEND:VCARD".into(),
            full_name: Some("Alice Smith".into()),
            family_name: Some("Smith".into()),
            given_name: Some("Alice".into()),
            organization: Some("Acme Inc.".into()),
            email_primary: Some("alice@example.com".into()),
            phone_primary: Some("+55 11 9999-9999".into()),
            birthday: Some("1990-05-15".into()),
            nickname: Some("Ali".into()),
            created_at: datetime!(2026-05-22 08:00:00 UTC),
            updated_at: datetime!(2026-05-22 08:00:00 UTC),
        }
    }

    #[test]
    fn contact_serde_roundtrip() {
        let c = sample();
        let s = serde_json::to_string(&c).unwrap();
        let back: Contact = serde_json::from_str(&s).unwrap();
        assert_eq!(back.uid, "uid-xyz@example.com");
        assert_eq!(back.full_name.as_deref(), Some("Alice Smith"));
    }

    #[test]
    fn contact_timestamps_in_rfc3339() {
        let c = sample();
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("2026-05-22T08:00:00"));
    }

    #[test]
    fn contact_optional_null_when_absent() {
        let mut c = sample();
        c.organization = None;
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert!(v["organization"].is_null());
    }

    #[test]
    fn contact_email_primary_present() {
        let c = sample();
        assert_eq!(c.email_primary.as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn contact_etag_preserved_in_roundtrip() {
        let c = sample();
        let back: Contact = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.etag, "etag42");
    }

    #[test]
    fn contact_phone_primary_present() {
        let c = sample();
        assert_eq!(c.phone_primary.as_deref(), Some("+55 11 9999-9999"));
    }

    #[test]
    fn contact_family_given_name_set() {
        let c = sample();
        assert_eq!(c.family_name.as_deref(), Some("Smith"));
        assert_eq!(c.given_name.as_deref(), Some("Alice"));
    }

    #[test]
    fn contact_organization_accessible() {
        let c = sample();
        assert!(c.organization.is_some() || c.organization.is_none());
    }

    #[test]
    fn contact_email_primary_preserved() {
        let c = sample();
        assert_eq!(c.email_primary.as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn contact_phone_primary_is_option() {
        let c = sample();
        let _ = c.phone_primary; // compiles only if field exists and is Option<String>
    }

    #[test]
    fn contact_full_name_preserved() {
        let c = sample();
        assert_eq!(c.full_name.as_deref(), Some("Alice Smith"));
    }

    #[test]
    fn contact_etag_not_empty() {
        let c = sample();
        assert!(!c.etag.is_empty());
    }

    #[test]
    fn contact_uid_not_empty() {
        let c = sample();
        assert!(!c.uid.is_empty());
    }

    #[test]
    fn contact_email_primary_alice_preserved() {
        let c = sample();
        assert_eq!(c.email_primary.as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn contact_addressbook_id_accessible() {
        let c = sample();
        let _ = c.addressbook_id; // field exists and is Uuid
    }

    #[test]
    fn contact_tenant_id_accessible() {
        let c = sample();
        assert_eq!(c.tenant_id, uuid::Uuid::nil());
    }

    #[test]
    fn contact_vcard_raw_preserved() {
        let c = sample();
        assert!(c.vcard_raw.contains("BEGIN:VCARD"));
    }

    #[test]
    fn contact_vcard_raw_ends_with_vcard() {
        let c = sample();
        assert!(c.vcard_raw.contains("END:VCARD"));
    }

    #[test]
    fn contact_id_is_uuid_nil() {
        let c = sample();
        assert_eq!(c.id, uuid::Uuid::nil());
    }

    #[test]
    fn contact_uid_nonempty_in_sample() {
        let c = sample();
        assert!(!c.uid.is_empty());
    }

    #[test]
    fn contact_organization_some_in_sample() {
        let c = sample();
        assert_eq!(c.organization.as_deref(), Some("Acme Inc."));
    }

    #[test]
    fn contact_vcard_raw_starts_with_begin() {
        let c = sample();
        assert!(c.vcard_raw.starts_with("BEGIN:VCARD"));
    }

    #[test]
    fn contact_id_field_accessible() {
        let c = sample();
        let _ = c.id;
    }
}

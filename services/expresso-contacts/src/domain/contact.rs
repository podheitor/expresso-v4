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
    pub id:             Uuid,
    pub addressbook_id: Uuid,
    pub tenant_id:      Uuid,
    pub uid:            String,
    pub etag:           String,
    pub vcard_raw:      String,
    pub full_name:      Option<String>,
    pub family_name:    Option<String>,
    pub given_name:     Option<String>,
    pub organization:   Option<String>,
    pub email_primary:  Option<String>,
    pub phone_primary:  Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:     OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at:     OffsetDateTime,
}

#[derive(Clone)]
pub struct ContactRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> ContactRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    /// Insert parsing raw vCard; UID uniqueness enforced by DB index.
    pub async fn create(
        &self,
        tenant_id:      Uuid,
        addressbook_id: Uuid,
        raw:            &str,
    ) -> Result<Contact> {
        let parsed = vcard::parse(raw).map_err(ContactsError::InvalidVCard)?;
        let etag   = vcard::compute_etag(raw);
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Contact>(
            r#"
            INSERT INTO contacts (
                addressbook_id, tenant_id, uid, etag, vcard_raw,
                full_name, family_name, given_name, organization,
                email_primary, phone_primary
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
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
        .fetch_one(&mut *tx)
        .await?;
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

    pub async fn update(
        &self,
        tenant_id: Uuid,
        id:        Uuid,
        raw:       &str,
    ) -> Result<Contact> {
        let parsed = vcard::parse(raw).map_err(ContactsError::InvalidVCard)?;
        let etag   = vcard::compute_etag(raw);
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
                phone_primary = $11
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
        .fetch_one(&mut *tx)
        .await?;
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
        tenant_id:      Uuid,
        addressbook_id: Uuid,
        uid:            &str,
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
        tenant_id:      Uuid,
        addressbook_id: Uuid,
        uids:           &[String],
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
        tenant_id:      Uuid,
        addressbook_id: Uuid,
        raw:            &str,
    ) -> Result<Contact> {
        let parsed = vcard::parse(raw).map_err(ContactsError::InvalidVCard)?;
        let etag   = vcard::compute_etag(raw);
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
        tenant_id:      Uuid,
        addressbook_id: Uuid,
        uid:            &str,
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
}

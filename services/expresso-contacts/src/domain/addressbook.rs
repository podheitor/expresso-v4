//! Addressbook collection — persistence layer (CardDAV-aware).
//!
//! Tenant scoping: cada método abre transação via `begin_tenant_tx` para que
//! a policy RLS de `addressbooks` filtre por `current_setting('app.tenant_id')`
//! antes mesmo do `WHERE tenant_id = $1` explícito (defense-in-depth).
//! Bonus em `access_level`: os dois SELECTs (owner_user_id + addressbook_acl)
//! agora rodam num snapshot consistente, evitando race em ACL handoff.

use expresso_core::{begin_tenant_tx, DbPool};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Addressbook {
    pub id:            Uuid,
    pub tenant_id:     Uuid,
    pub owner_user_id: Uuid,
    pub name:          String,
    pub description:   Option<String>,
    pub ctag:          i64,
    pub is_default:    bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:    OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at:    OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewAddressbook {
    pub name:        String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub is_default:  bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UpdateAddressbook {
    #[serde(default)]
    pub name:        Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub is_default:  Option<bool>,
}

#[derive(Clone)]
pub struct AddressbookRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> AddressbookRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    pub async fn create(
        &self,
        tenant_id: Uuid,
        owner_user_id: Uuid,
        input: NewAddressbook,
    ) -> Result<Addressbook> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Addressbook>(
            r#"
            INSERT INTO addressbooks (tenant_id, owner_user_id, name, description, is_default)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING *
            "#,
        )
        .bind(tenant_id)
        .bind(owner_user_id)
        .bind(input.name)
        .bind(input.description)
        .bind(input.is_default)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }


    /// Insere addressbook honrando UUID fornecido (CardDAV MKCOL).
    pub async fn create_with_id(
        &self,
        id: Uuid,
        tenant_id: Uuid,
        owner_user_id: Uuid,
        input: NewAddressbook,
    ) -> Result<Addressbook> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Addressbook>(
            r#"
            INSERT INTO addressbooks (id, tenant_id, owner_user_id, name, description, is_default)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(tenant_id)
        .bind(owner_user_id)
        .bind(input.name)
        .bind(input.description)
        .bind(input.is_default)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn list_for_owner(&self, tenant_id: Uuid, owner: Uuid) -> Result<Vec<Addressbook>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query_as::<_, Addressbook>(
            r#"SELECT * FROM addressbooks
               WHERE tenant_id = $1 AND owner_user_id = $2
               ORDER BY is_default DESC, name"#,
        )
        .bind(tenant_id)
        .bind(owner)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn get(&self, tenant_id: Uuid, id: Uuid) -> Result<Addressbook> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Addressbook>(
            r#"SELECT * FROM addressbooks WHERE tenant_id = $1 AND id = $2"#,
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn update(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        input: UpdateAddressbook,
    ) -> Result<Addressbook> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let row = sqlx::query_as::<_, Addressbook>(
            r#"
            UPDATE addressbooks SET
                name        = COALESCE($3, name),
                description = COALESCE($4, description),
                is_default  = COALESCE($5, is_default)
            WHERE tenant_id = $1 AND id = $2
            RETURNING *
            "#,
        )
        .bind(tenant_id)
        .bind(id)
        .bind(input.name)
        .bind(input.description)
        .bind(input.is_default)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn delete(&self, tenant_id: Uuid, id: Uuid) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        sqlx::query(r#"DELETE FROM addressbooks WHERE tenant_id = $1 AND id = $2"#)
            .bind(tenant_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn ctag(&self, tenant_id: Uuid, id: Uuid) -> Result<i64> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let (ctag,): (i64,) = sqlx::query_as(
            r#"SELECT ctag FROM addressbooks WHERE tenant_id = $1 AND id = $2"#,
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ctag)
    }

    /// Owned + shared via addressbook_acl.
    pub async fn list_accessible(&self, tenant_id: Uuid, user_id: Uuid) -> Result<Vec<Addressbook>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query_as::<_, Addressbook>(
            r#"SELECT * FROM addressbooks
               WHERE tenant_id = $1
                 AND (owner_user_id = $2
                      OR id IN (SELECT addressbook_id FROM addressbook_acl
                                 WHERE tenant_id = $1 AND grantee_id = $2))
               ORDER BY is_default DESC, name"#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// "OWNER" | "READ" | "WRITE" | "ADMIN" | None.
    ///
    /// Os dois SELECTs (owner em `addressbooks` + privilege em `addressbook_acl`)
    /// rodam na MESMA tx, dentro do mesmo snapshot — evita race em ACL handoff
    /// (ex.: transferência de owner concorrente com revoke de ACL).
    pub async fn access_level(
        &self,
        tenant_id: Uuid,
        addrbook_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<String>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let owner: Option<(Uuid,)> = sqlx::query_as(
            "SELECT owner_user_id FROM addressbooks WHERE id = $1 AND tenant_id = $2",
        )
        .bind(addrbook_id)
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
        let acl: Option<(String,)> = sqlx::query_as(
            "SELECT privilege FROM addressbook_acl
              WHERE addressbook_id = $1 AND tenant_id = $2 AND grantee_id = $3",
        )
        .bind(addrbook_id)
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(acl.map(|(p,)| p))
    }
}


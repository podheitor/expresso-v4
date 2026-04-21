//! Addressbook collection — persistence layer (CardDAV-aware).

use expresso_core::DbPool;
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
        .fetch_one(self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list_for_owner(&self, tenant_id: Uuid, owner: Uuid) -> Result<Vec<Addressbook>> {
        let rows = sqlx::query_as::<_, Addressbook>(
            r#"SELECT * FROM addressbooks
               WHERE tenant_id = $1 AND owner_user_id = $2
               ORDER BY is_default DESC, name"#,
        )
        .bind(tenant_id)
        .bind(owner)
        .fetch_all(self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get(&self, tenant_id: Uuid, id: Uuid) -> Result<Addressbook> {
        let row = sqlx::query_as::<_, Addressbook>(
            r#"SELECT * FROM addressbooks WHERE tenant_id = $1 AND id = $2"#,
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_one(self.pool)
        .await?;
        Ok(row)
    }

    pub async fn update(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        input: UpdateAddressbook,
    ) -> Result<Addressbook> {
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
        .fetch_one(self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete(&self, tenant_id: Uuid, id: Uuid) -> Result<()> {
        sqlx::query(r#"DELETE FROM addressbooks WHERE tenant_id = $1 AND id = $2"#)
            .bind(tenant_id)
            .bind(id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn ctag(&self, tenant_id: Uuid, id: Uuid) -> Result<i64> {
        let (ctag,): (i64,) = sqlx::query_as(
            r#"SELECT ctag FROM addressbooks WHERE tenant_id = $1 AND id = $2"#,
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_one(self.pool)
        .await?;
        Ok(ctag)
    }
}

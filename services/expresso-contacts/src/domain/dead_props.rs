//! WebDAV RFC 4918 §15 "dead" property store — v1 scope: addressbook collection.

use expresso_core::DbPool;
use sqlx::Row;
use uuid::Uuid;

use crate::error::Result;

#[derive(Debug, Clone)]
pub struct DeadProp {
    pub namespace:  String,
    pub local_name: String,
    pub xml_value:  String,
}

pub struct DeadPropRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> DeadPropRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self { Self { pool } }

    pub async fn upsert_addressbook(
        &self,
        tenant_id:      Uuid,
        addressbook_id: Uuid,
        namespace:      &str,
        local_name:     &str,
        value:          &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO addressbook_dead_properties
                (tenant_id, addressbook_id, namespace, local_name, xml_value)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (addressbook_id, namespace, local_name)
            DO UPDATE SET xml_value = EXCLUDED.xml_value, updated_at = now()
            "#,
        )
        .bind(tenant_id)
        .bind(addressbook_id)
        .bind(namespace)
        .bind(local_name)
        .bind(value)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn remove_addressbook(
        &self,
        addressbook_id: Uuid,
        namespace:      &str,
        local_name:     &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM addressbook_dead_properties
            WHERE addressbook_id = $1 AND namespace = $2 AND local_name = $3
            "#,
        )
        .bind(addressbook_id)
        .bind(namespace)
        .bind(local_name)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_for_addressbook(
        &self,
        addressbook_id: Uuid,
    ) -> Result<Vec<DeadProp>> {
        let rows = sqlx::query(
            r#"
            SELECT namespace, local_name, xml_value
            FROM addressbook_dead_properties
            WHERE addressbook_id = $1
            ORDER BY namespace, local_name
            "#,
        )
        .bind(addressbook_id)
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| DeadProp {
                namespace:  r.get("namespace"),
                local_name: r.get("local_name"),
                xml_value:  r.get("xml_value"),
            })
            .collect())
    }
}

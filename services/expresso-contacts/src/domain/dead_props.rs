//! WebDAV RFC 4918 §15 "dead" property store — v1 scope: addressbook collection.
//!
//! Tenant scoping: cada método abre transação via `begin_tenant_tx` e filtra
//! `WHERE tenant_id = $1 AND addressbook_id = $2`. `remove_addressbook` e
//! `list_for_addressbook` passaram a receber `tenant_id` (API change) —
//! antes atualizavam/liam só por `addressbook_id`, sem guardrail. Migration
//! de `addressbook_dead_properties` ainda não tem ENABLE ROW LEVEL SECURITY;
//! `begin_tenant_tx` fica pronto p/ quando a RLS for adicionada.

use expresso_core::{begin_tenant_tx, DbPool};
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
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
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
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// API: `tenant_id` now required — antes filtrava só por addressbook_id.
    pub async fn remove_addressbook(
        &self,
        tenant_id:      Uuid,
        addressbook_id: Uuid,
        namespace:      &str,
        local_name:     &str,
    ) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        sqlx::query(
            r#"
            DELETE FROM addressbook_dead_properties
            WHERE tenant_id = $1 AND addressbook_id = $2
              AND namespace = $3 AND local_name = $4
            "#,
        )
        .bind(tenant_id)
        .bind(addressbook_id)
        .bind(namespace)
        .bind(local_name)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// API: `tenant_id` now required — antes filtrava só por addressbook_id.
    pub async fn list_for_addressbook(
        &self,
        tenant_id:      Uuid,
        addressbook_id: Uuid,
    ) -> Result<Vec<DeadProp>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query(
            r#"
            SELECT namespace, local_name, xml_value
            FROM addressbook_dead_properties
            WHERE tenant_id = $1 AND addressbook_id = $2
            ORDER BY namespace, local_name
            "#,
        )
        .bind(tenant_id)
        .bind(addressbook_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_prop_clone_preserves_fields() {
        let p = DeadProp {
            namespace:  "urn:ietf:params:xml:ns:carddav".into(),
            local_name: "addressbook-description".into(),
            xml_value:  "My addressbook".into(),
        };
        let cloned = p.clone();
        assert_eq!(cloned.namespace, p.namespace);
        assert_eq!(cloned.local_name, p.local_name);
        assert_eq!(cloned.xml_value, p.xml_value);
    }

    #[test]
    fn dead_prop_debug_contains_namespace() {
        let p = DeadProp {
            namespace:  "DAV:".into(),
            local_name: "displayname".into(),
            xml_value:  "Contacts".into(),
        };
        let dbg = format!("{p:?}");
        assert!(dbg.contains("DAV:"));
    }

    #[test]
    fn dead_prop_empty_xml_value_allowed() {
        let p = DeadProp {
            namespace: "http://carddav.example.com/".into(),
            local_name: "vcard-data".into(),
            xml_value: "".into(),
        };
        assert!(p.xml_value.is_empty());
    }

    #[test]
    fn dead_prop_long_value_preserved() {
        let long = "x".repeat(2048);
        let p = DeadProp {
            namespace: "DAV:".into(),
            local_name: "prop-with-long-value".into(),
            xml_value: long.clone(),
        };
        assert_eq!(p.xml_value, long);
    }
}

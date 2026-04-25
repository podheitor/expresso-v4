//! WebDAV RFC 4918 §15 "dead" property store — v1 scope: calendar collection.
//!
//! Arbitrary client-supplied (`namespace`, `local_name`) pairs preserved
//! verbatim across requests. Storage is text-only; no XML mixed content.
//!
//! Tenant scoping: cada método abre transação via `begin_tenant_tx` e
//! filtra `WHERE tenant_id = $1 AND calendar_id = $2`. `remove_calendar`
//! e `list_for_calendar` passaram a receber `tenant_id` (API change) —
//! antes atualizavam/liam só por `calendar_id`, sem guardrail. Migration
//! de `calendar_dead_properties` ainda não tem ENABLE ROW LEVEL SECURITY;
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

    /// Upsert a dead property. `value` is escaped text content.
    pub async fn upsert_calendar(
        &self,
        tenant_id:   Uuid,
        calendar_id: Uuid,
        namespace:   &str,
        local_name:  &str,
        value:       &str,
    ) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        sqlx::query(
            r#"
            INSERT INTO calendar_dead_properties
                (tenant_id, calendar_id, namespace, local_name, xml_value)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (calendar_id, namespace, local_name)
            DO UPDATE SET xml_value = EXCLUDED.xml_value, updated_at = now()
            "#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(namespace)
        .bind(local_name)
        .bind(value)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Delete a dead property (no-op if absent).
    /// API: `tenant_id` now required — antes filtrava só por calendar_id.
    pub async fn remove_calendar(
        &self,
        tenant_id:   Uuid,
        calendar_id: Uuid,
        namespace:   &str,
        local_name:  &str,
    ) -> Result<()> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        sqlx::query(
            r#"
            DELETE FROM calendar_dead_properties
            WHERE tenant_id = $1 AND calendar_id = $2
              AND namespace = $3 AND local_name = $4
            "#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
        .bind(namespace)
        .bind(local_name)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// List all dead props for a calendar (for allprop PROPFIND).
    /// API: `tenant_id` now required — antes filtrava só por calendar_id.
    pub async fn list_for_calendar(
        &self,
        tenant_id:   Uuid,
        calendar_id: Uuid,
    ) -> Result<Vec<DeadProp>> {
        let mut tx = begin_tenant_tx(self.pool, tenant_id).await?;
        let rows = sqlx::query(
            r#"
            SELECT namespace, local_name, xml_value
            FROM calendar_dead_properties
            WHERE tenant_id = $1 AND calendar_id = $2
            ORDER BY namespace, local_name
            "#,
        )
        .bind(tenant_id)
        .bind(calendar_id)
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

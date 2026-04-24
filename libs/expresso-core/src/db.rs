//! PostgreSQL connection pool + tenant RLS context helper

use sqlx::{PgPool, PgConnection, Postgres, Transaction};
use std::time::Duration;
use uuid::Uuid;
use crate::{config::DatabaseConfig, error::{CoreError, Result}};

pub type DbPool = PgPool;

/// Build a PgPool from config.
pub async fn create_pool(cfg: &DatabaseConfig) -> Result<DbPool> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(cfg.max_connections)
        .min_connections(cfg.min_connections)
        .acquire_timeout(Duration::from_secs(cfg.acquire_timeout_secs))
        .connect(&cfg.url)
        .await
        .map_err(CoreError::Database)?;

    Ok(pool)
}

/// Set the current tenant in the PostgreSQL session for RLS.
/// Note: `set_config(name, value, is_local=true)` only persists for the
/// current transaction. Outside a `BEGIN`, the setting reverts immediately
/// once the implicit single-statement tx commits — RLS policies that
/// depend on `current_setting('app.tenant_id', true)` will then see an
/// empty string. Prefer `begin_tenant_tx` for handlers that touch
/// FORCE-RLS tables.
pub async fn set_tenant_context(conn: &mut PgConnection, tenant_id: Uuid) -> Result<()> {
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant_id.to_string())
        .execute(conn)
        .await
        .map_err(CoreError::Database)?;

    Ok(())
}

/// Open a transaction with `app.tenant_id` already populated, ready for
/// queries against FORCE-RLS tables. Caller must `tx.commit().await` (or
/// drop to rollback). Defense-in-depth on top of explicit `WHERE tenant_id`
/// clauses in repository code.
pub async fn begin_tenant_tx(pool: &DbPool, tenant_id: Uuid) -> Result<Transaction<'_, Postgres>> {
    let mut tx = pool.begin().await.map_err(CoreError::Database)?;
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(CoreError::Database)?;
    Ok(tx)
}

/// Snapshot of RLS-relevant DB role/table state. Rendered at startup so
/// operators can confirm the actual security posture rather than guessing
/// from migration source.
#[derive(Debug, Clone)]
pub struct RlsPosture {
    pub role:           String,
    pub bypassrls:      bool,
    pub tables_missing: Vec<String>,
    pub tables_unforced: Vec<String>,
}

impl RlsPosture {
    /// True when every checked table has RLS enabled + forced AND the
    /// current role does not bypass RLS (i.e. policies actually apply).
    pub fn is_strict(&self) -> bool {
        !self.bypassrls
            && self.tables_missing.is_empty()
            && self.tables_unforced.is_empty()
    }
}

/// Inspect `pg_roles` + `pg_class` to report RLS posture for the listed
/// tables (public schema). Logs a single INFO line on success or WARN on
/// any deviation; never fails the caller.
pub async fn report_rls_posture(pool: &DbPool, tables: &[&str]) -> RlsPosture {
    let role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(pool).await.unwrap_or_else(|_| "unknown".into());
    let bypassrls: bool = sqlx::query_scalar(
        "SELECT COALESCE((SELECT rolbypassrls FROM pg_roles WHERE rolname = current_user), false)"
    ).fetch_one(pool).await.unwrap_or(false);

    // relrowsecurity = ENABLE RLS, relforcerowsecurity = FORCE RLS.
    let rows: Vec<(String, bool, bool)> = sqlx::query_as(
        "SELECT c.relname::text, c.relrowsecurity, c.relforcerowsecurity \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relname = ANY($1)"
    )
    .bind(tables)
    .fetch_all(pool).await.unwrap_or_default();

    let present: std::collections::HashMap<&str, (bool, bool)> = rows.iter()
        .map(|(n, e, f)| (n.as_str(), (*e, *f))).collect();

    let mut tables_missing  = Vec::new();
    let mut tables_unforced = Vec::new();
    for t in tables {
        match present.get(t) {
            None                  => tables_missing.push((*t).to_string()),
            Some((false, _))      => tables_unforced.push(format!("{t} (RLS disabled)")),
            Some((true,  false))  => tables_unforced.push(format!("{t} (not FORCEd)")),
            Some((true,  true))   => {} // OK
        }
    }

    let posture = RlsPosture { role, bypassrls, tables_missing, tables_unforced };
    if posture.is_strict() {
        tracing::info!(role = %posture.role, tables = tables.len(), "RLS posture: strict");
    } else {
        tracing::warn!(
            role = %posture.role,
            bypassrls = posture.bypassrls,
            missing  = ?posture.tables_missing,
            unforced = ?posture.tables_unforced,
            "RLS posture: NOT strict — tenant isolation depends on application-level WHERE clauses",
        );
    }
    posture
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> RlsPosture {
        RlsPosture {
            role: "expresso".into(),
            bypassrls: false,
            tables_missing: vec![],
            tables_unforced: vec![],
        }
    }

    #[test]
    fn strict_when_clean() {
        assert!(ok().is_strict());
    }

    #[test]
    fn not_strict_with_bypassrls() {
        let mut p = ok();
        p.bypassrls = true;
        assert!(!p.is_strict());
    }

    #[test]
    fn not_strict_with_missing_table() {
        let mut p = ok();
        p.tables_missing.push("drive_files".into());
        assert!(!p.is_strict());
    }

    #[test]
    fn not_strict_when_not_forced() {
        let mut p = ok();
        p.tables_unforced.push("drive_uploads (not FORCEd)".into());
        assert!(!p.is_strict());
    }
}

/// Run pending sqlx migrations from the `./migrations` directory.
pub async fn run_migrations(pool: &DbPool) -> Result<()> {
    sqlx::migrate!("../../migrations")
        .run(pool)
        .await
        .map_err(|e| CoreError::Database(e.into()))?;

    Ok(())
}

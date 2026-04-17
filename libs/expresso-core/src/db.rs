//! PostgreSQL connection pool + tenant RLS context helper

use sqlx::{PgPool, PgConnection};
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
/// Must be called at the start of every request handler.
///
/// Usage: `set_tenant_context(&mut conn, tenant_id).await?;`
pub async fn set_tenant_context(conn: &mut PgConnection, tenant_id: Uuid) -> Result<()> {
    // SET LOCAL scopes to the current transaction
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant_id.to_string())
        .execute(conn)
        .await
        .map_err(CoreError::Database)?;

    Ok(())
}

/// Run pending sqlx migrations from the `./migrations` directory.
pub async fn run_migrations(pool: &DbPool) -> Result<()> {
    sqlx::migrate!("../../migrations")
        .run(pool)
        .await
        .map_err(|e| CoreError::Database(e.into()))?;

    Ok(())
}

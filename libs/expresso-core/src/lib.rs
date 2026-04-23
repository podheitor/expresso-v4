//! expresso-core: Shared config, DB pool, Redis, error types, telemetry

pub mod config;
pub mod db;
pub mod error;
pub mod redis;
pub mod audit;
pub mod telemetry;

// Re-export most-used types at crate root
pub use config::AppConfig;
pub use db::{DbPool, create_pool as create_db_pool, set_tenant_context, run_migrations};
pub use redis::{RedisPool, create_pool as create_redis_pool};
pub use error::{CoreError, Result};
pub use telemetry::init_tracing;

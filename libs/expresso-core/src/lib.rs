//! expresso-core: Shared config, DB pool, Redis, error types, telemetry

pub mod audit;
pub mod config;
pub mod db;
pub mod error;
pub mod health;
pub mod ratelimit;
pub mod redis;
pub mod telemetry;

// Re-export most-used types at crate root
pub use config::AppConfig;
pub use db::{
    begin_tenant_tx, create_pool as create_db_pool, report_rls_posture, run_migrations,
    set_tenant_context, DbPool, RlsPosture,
};
pub use error::{CoreError, Result};
pub use ratelimit::{RateLimitConfig, RateLimiter};
pub use redis::{create_pool as create_redis_pool, RedisPool};
pub use telemetry::init_tracing;

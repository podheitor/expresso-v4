use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),

    #[error("redis pool error: {0}")]
    RedisPool(#[from] deadpool_redis::PoolError),

    #[error("configuration error: {0}")]
    Config(#[from] config::ConfigError),

    #[error("tenant not set in context")]
    TenantNotSet,

    #[error("not found: {resource}")]
    NotFound { resource: &'static str },

    #[error("quota exceeded: used {used} of {limit} bytes")]
    QuotaExceeded { used: i64, limit: i64 },

    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, CoreError>;

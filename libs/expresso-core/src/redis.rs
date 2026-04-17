//! Redis connection pool via deadpool-redis

use deadpool_redis::{Config as RedisPoolConfig, Pool};
use crate::{config::RedisConfig, error::{CoreError, Result}};

pub type RedisPool = Pool;

/// Build a deadpool-redis pool from config.
pub fn create_pool(cfg: &RedisConfig) -> Result<RedisPool> {
    let pool = RedisPoolConfig::from_url(&cfg.url)
        .builder()
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("redis pool builder: {e}")))?
        .max_size(cfg.pool_size)
        .build()
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("redis pool create: {e}")))?;

    Ok(pool)
}


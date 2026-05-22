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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_not_set_display() {
        assert_eq!(CoreError::TenantNotSet.to_string(), "tenant not set in context");
    }

    #[test]
    fn not_found_display_contains_resource() {
        let e = CoreError::NotFound { resource: "drive_file" };
        assert!(e.to_string().contains("drive_file"));
    }

    #[test]
    fn quota_exceeded_display_contains_used_and_limit() {
        let e = CoreError::QuotaExceeded { used: 512, limit: 1024 };
        let s = e.to_string();
        assert!(s.contains("512") && s.contains("1024"));
    }

    #[test]
    fn quota_exceeded_zero_used() {
        let e = CoreError::QuotaExceeded { used: 0, limit: 100 };
        assert!(e.to_string().contains('0'));
    }
}

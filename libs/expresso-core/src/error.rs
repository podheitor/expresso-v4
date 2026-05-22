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

    #[test]
    fn tenant_not_set_display_via_let() {
        let e = CoreError::TenantNotSet;
        assert_eq!(e.to_string(), "tenant not set in context");
    }

    #[test]
    fn internal_error_display() {
        let e = CoreError::Internal(anyhow::anyhow!("something went wrong"));
        assert!(e.to_string().contains("something went wrong"));
    }

    #[test]
    fn not_found_mail_message() {
        let e = CoreError::NotFound { resource: "mail_message" };
        assert!(e.to_string().contains("mail_message"));
    }

    #[test]
    fn quota_exceeded_fields_in_display() {
        let e = CoreError::QuotaExceeded { used: 999, limit: 1000 };
        let s = e.to_string();
        assert!(s.contains("999") && s.contains("1000"));
    }

    #[test]
    fn not_found_calendar_event_in_display() {
        let e = CoreError::NotFound { resource: "calendar_event" };
        assert!(e.to_string().contains("calendar_event"));
    }

    #[test]
    fn quota_exceeded_equal_used_limit_in_display() {
        let e = CoreError::QuotaExceeded { used: 1024, limit: 1024 };
        let s = e.to_string();
        assert!(s.contains("1024"));
    }

    #[test]
    fn core_error_internal_display_contains_message() {
        let e = CoreError::Internal(anyhow::anyhow!("db pool exhausted"));
        assert!(e.to_string().contains("db pool exhausted"));
    }

    #[test]
    fn core_error_not_found_display_contains_resource() {
        let e = CoreError::NotFound { resource: "calendar_event" };
        assert!(e.to_string().contains("calendar_event"));
    }

    #[test]
    fn core_error_tenant_not_set_display_not_empty() {
        let e = CoreError::TenantNotSet;
        assert!(!e.to_string().is_empty());
    }
}

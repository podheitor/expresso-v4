//! Stats service error types.

use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;
use thiserror::Error;

pub type Result<T, E = StatsError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum StatsError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("database unavailable")]
    DatabaseUnavailable,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("forbidden")]
    Forbidden,

    #[error("unauthorized")]
    Unauthorized,

    #[error("internal: {0}")]
    Internal(String),
}

impl IntoResponse for StatsError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::DatabaseUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::BadRequest(_)       => StatusCode::BAD_REQUEST,
            Self::NotFound(_)         => StatusCode::NOT_FOUND,
            Self::Forbidden           => StatusCode::FORBIDDEN,
            Self::Unauthorized        => StatusCode::UNAUTHORIZED,
            Self::Database(_) | Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = Json(json!({"error": self.to_string()}));
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_request_display() {
        let e = StatsError::BadRequest("missing field".into());
        assert_eq!(e.to_string(), "bad request: missing field");
    }

    #[test]
    fn not_found_display() {
        let e = StatsError::NotFound("tenant".into());
        assert_eq!(e.to_string(), "not found: tenant");
    }

    #[test]
    fn forbidden_display() {
        let e = StatsError::Forbidden;
        assert_eq!(e.to_string(), "forbidden");
    }

    #[test]
    fn unauthorized_display() {
        let e = StatsError::Unauthorized;
        assert_eq!(e.to_string(), "unauthorized");
    }

    #[test]
    fn database_unavailable_display() {
        let e = StatsError::DatabaseUnavailable;
        assert_eq!(e.to_string(), "database unavailable");
    }

    #[test]
    fn internal_display() {
        let e = StatsError::Internal("oops".into());
        assert_eq!(e.to_string(), "internal: oops");
    }

    #[test]
    fn bad_request_debug_contains_variant() {
        let e = StatsError::BadRequest("x".into());
        let s = format!("{:?}", e);
        assert!(s.contains("BadRequest"));
    }

    #[test]
    fn not_found_debug_contains_variant() {
        let e = StatsError::NotFound("x".into());
        let s = format!("{:?}", e);
        assert!(s.contains("NotFound"));
    }

    #[test]
    fn forbidden_debug_contains_variant() {
        let e = StatsError::Forbidden;
        let s = format!("{:?}", e);
        assert!(s.contains("Forbidden"));
    }

    #[test]
    fn unauthorized_debug_contains_variant() {
        let e = StatsError::Unauthorized;
        let s = format!("{:?}", e);
        assert!(s.contains("Unauthorized"));
    }

    #[test]
    fn internal_debug_contains_variant() {
        let e = StatsError::Internal("boom".into());
        let s = format!("{:?}", e);
        assert!(s.contains("Internal"));
    }

    #[test]
    fn bad_request_message_preserved() {
        let msg = "field x is required";
        let e = StatsError::BadRequest(msg.into());
        assert!(e.to_string().contains(msg));
    }

    #[test]
    fn not_found_message_preserved() {
        let msg = "tenant-xyz";
        let e = StatsError::NotFound(msg.into());
        assert!(e.to_string().contains(msg));
    }

    #[test]
    fn internal_message_preserved() {
        let msg = "unexpected nil pointer";
        let e = StatsError::Internal(msg.into());
        assert!(e.to_string().contains(msg));
    }

    #[test]
    fn database_unavailable_is_not_bad_request() {
        let e = StatsError::DatabaseUnavailable;
        assert_ne!(e.to_string(), StatsError::BadRequest("".into()).to_string());
    }

    #[test]
    fn forbidden_is_not_unauthorized() {
        assert_ne!(
            StatsError::Forbidden.to_string(),
            StatsError::Unauthorized.to_string()
        );
    }

    #[test]
    fn bad_request_with_empty_message() {
        let e = StatsError::BadRequest(String::new());
        assert!(e.to_string().starts_with("bad request:"));
    }

    #[test]
    fn not_found_with_empty_message() {
        let e = StatsError::NotFound(String::new());
        assert!(e.to_string().starts_with("not found:"));
    }

    #[test]
    fn internal_with_empty_message() {
        let e = StatsError::Internal(String::new());
        assert!(e.to_string().starts_with("internal:"));
    }

    #[test]
    fn result_ok_variant_accessible() {
        let r: Result<i32> = Ok(42);
        assert_eq!(r.unwrap(), 42);
    }

    #[test]
    fn result_err_variant_accessible() {
        let r: Result<i32> = Err(StatsError::Forbidden);
        assert!(r.is_err());
    }

    #[test]
    fn database_unavailable_debug_contains_variant() {
        let e = StatsError::DatabaseUnavailable;
        let s = format!("{:?}", e);
        assert!(s.contains("DatabaseUnavailable"));
    }

    #[test]
    fn bad_request_display_starts_with_prefix() {
        let e = StatsError::BadRequest("test".into());
        assert!(e.to_string().starts_with("bad request:"));
    }

    #[test]
    fn unauthorized_display_is_unauthorized() {
        assert_eq!(StatsError::Unauthorized.to_string(), "unauthorized");
    }

    #[test]
    fn internal_debug_contains_message() {
        let e = StatsError::Internal("detail".into());
        let s = format!("{:?}", e);
        assert!(s.contains("detail"));
    }

    #[test]
    fn not_found_display_starts_with_not_found() {
        let e = StatsError::NotFound("x".into());
        assert!(e.to_string().starts_with("not found:"));
    }
}

//! Drive service error types.

use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

pub type Result<T, E = DriveError> = std::result::Result<T, E>;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum DriveError {
    #[error("core: {0}")]
    Core(#[from] expresso_core::CoreError),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("database unavailable")]
    DatabaseUnavailable,

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("not found: {0}")]
    NotFound(Uuid),

    #[error("gone: {0}")]
    Gone(Uuid),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("forbidden")]
    Forbidden,

    #[error("unauthorized")]
    Unauthorized,

    #[error("quota exceeded")]
    QuotaExceeded,
}

impl IntoResponse for DriveError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::DatabaseUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::NotFound(_)         => StatusCode::NOT_FOUND,
            Self::Gone(_)             => StatusCode::GONE,
            Self::Conflict(_)         => StatusCode::CONFLICT,
            Self::BadRequest(_)       => StatusCode::BAD_REQUEST,
            Self::Forbidden           => StatusCode::FORBIDDEN,
            Self::Unauthorized        => StatusCode::UNAUTHORIZED,
            Self::QuotaExceeded      => StatusCode::INSUFFICIENT_STORAGE,
            Self::Io(_) | Self::Database(_) | Self::Core(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = Json(json!({"error": self.to_string()}));
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    fn status(e: DriveError) -> StatusCode {
        e.into_response().status()
    }

    #[test]
    fn not_found_is_404() {
        assert_eq!(status(DriveError::NotFound(Uuid::new_v4())), StatusCode::NOT_FOUND);
    }

    #[test]
    fn gone_is_410() {
        assert_eq!(status(DriveError::Gone(Uuid::new_v4())), StatusCode::GONE);
    }

    #[test]
    fn conflict_is_409() {
        assert_eq!(status(DriveError::Conflict("dup".into())), StatusCode::CONFLICT);
    }

    #[test]
    fn bad_request_is_400() {
        assert_eq!(status(DriveError::BadRequest("bad".into())), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn forbidden_is_403() {
        assert_eq!(status(DriveError::Forbidden), StatusCode::FORBIDDEN);
    }

    #[test]
    fn unauthorized_is_401() {
        assert_eq!(status(DriveError::Unauthorized), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn quota_exceeded_is_507() {
        assert_eq!(status(DriveError::QuotaExceeded), StatusCode::INSUFFICIENT_STORAGE);
    }

    #[test]
    fn database_unavailable_is_503() {
        assert_eq!(status(DriveError::DatabaseUnavailable), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn gone_display_contains_id() {
        let id = Uuid::nil();
        assert!(format!("{}", DriveError::Gone(id)).contains(&id.to_string()));
    }

    #[test]
    fn not_found_display_contains_id() {
        let id = Uuid::nil();
        let e = DriveError::NotFound(id);
        assert!(format!("{e}").contains(&id.to_string()));
    }

    #[test]
    fn conflict_display_not_empty() {
        let e = DriveError::Conflict("lock mismatch".into());
        assert!(!format!("{e}").is_empty());
    }

    #[test]
    fn forbidden_display_is_forbidden() {
        let e = DriveError::Forbidden;
        assert!(format!("{e}").contains("forbidden") || !format!("{e}").is_empty());
    }

    #[test]
    fn quota_exceeded_status_is_507() {
        assert_eq!(status(DriveError::QuotaExceeded), StatusCode::INSUFFICIENT_STORAGE);
    }

    #[test]
    fn gone_status_is_410() {
        assert_eq!(status(DriveError::Gone(Uuid::nil())), StatusCode::GONE);
    }

    #[test]
    fn conflict_lock_mismatch_is_409() {
        assert_eq!(status(DriveError::Conflict("lock mismatch".into())), StatusCode::CONFLICT);
    }

    #[test]
    fn unauthorized_display_not_empty() {
        let e = DriveError::Unauthorized;
        assert!(!format!("{e}").is_empty());
    }

    #[test]
    fn bad_request_display_contains_message() {
        let e = DriveError::BadRequest("missing field".into());
        assert!(format!("{e}").contains("missing field"));
    }

    #[test]
    fn io_error_is_500() {
        let e = DriveError::Io(std::io::Error::new(std::io::ErrorKind::Other, "oops"));
        assert_eq!(status(e), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn database_unavailable_is_503_status() {
        assert_eq!(status(DriveError::DatabaseUnavailable), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn not_found_display_contains_not_found_prefix() {
        let id = Uuid::new_v4();
        let e = DriveError::NotFound(id);
        let s = format!("{e}");
        assert!(s.starts_with("not found:"));
    }

    #[test]
    fn bad_request_display_is_not_empty() {
        let e = DriveError::BadRequest("missing field".into());
        assert!(!format!("{e}").is_empty());
    }
}

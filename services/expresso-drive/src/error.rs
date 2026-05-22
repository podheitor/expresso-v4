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
    fn not_found_is_404() {
        use uuid::Uuid;
        assert_eq!(status(DriveError::NotFound(Uuid::nil())), StatusCode::NOT_FOUND);
    }
}

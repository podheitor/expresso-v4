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

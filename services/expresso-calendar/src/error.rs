//! Calendar service error types

use axum::{response::{IntoResponse, Response}, http::StatusCode, Json};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

pub type Result<T, E = CalendarError> = std::result::Result<T, E>;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum CalendarError {
    #[error("core: {0}")]
    Core(#[from] expresso_core::CoreError),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("database unavailable")]
    DatabaseUnavailable,

    #[error("event not found: {0}")]
    EventNotFound(Uuid),

    #[error("calendar not found: {0}")]
    CalendarNotFound(String),

    #[error("invalid iCal data: {0}")]
    InvalidICal(String),

    #[error("scheduling conflict")]
    Conflict,

    #[error("forbidden")]
    Forbidden,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("not supported: {0}")]
    NotSupported(&'static str),
}

impl IntoResponse for CalendarError {
    fn into_response(self) -> Response {
        let (status, code, msg) = match &self {
            Self::EventNotFound(_)     => (StatusCode::NOT_FOUND,             "event_not_found",     self.to_string()),
            Self::CalendarNotFound(_)  => (StatusCode::NOT_FOUND,             "calendar_not_found",  self.to_string()),
            Self::InvalidICal(_)       => (StatusCode::BAD_REQUEST,           "invalid_ical",        self.to_string()),
            Self::BadRequest(_)        => (StatusCode::BAD_REQUEST,           "bad_request",         self.to_string()),
            Self::Conflict             => (StatusCode::CONFLICT,              "scheduling_conflict", self.to_string()),
            Self::Forbidden            => (StatusCode::FORBIDDEN,             "forbidden",           self.to_string()),
            Self::DatabaseUnavailable  => (StatusCode::SERVICE_UNAVAILABLE,   "db_unavailable",      self.to_string()),
            Self::NotSupported(_)      => (StatusCode::NOT_IMPLEMENTED,       "not_supported",       self.to_string()),
            // Unique violation → 409, FK violation / not-found → 404, everything else → 500.
            Self::Database(sqlx::Error::Database(db_err)) if db_err.is_unique_violation()
                                       => (StatusCode::CONFLICT,              "unique_violation",    "recurso duplicado".into()),
            Self::Database(sqlx::Error::RowNotFound)
                                       => (StatusCode::NOT_FOUND,             "not_found",           "recurso não encontrado".into()),
            Self::Database(_)          => (StatusCode::INTERNAL_SERVER_ERROR, "database",            "erro interno".into()),
            Self::Core(_)              => (StatusCode::INTERNAL_SERVER_ERROR, "internal",            "erro interno".into()),
        };
        (status, Json(json!({"error": code, "message": msg}))).into_response()
    }
}

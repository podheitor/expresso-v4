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
}

impl IntoResponse for CalendarError {
    fn into_response(self) -> Response {
        let (status, code, msg) = match &self {
            Self::EventNotFound(_)    => (StatusCode::NOT_FOUND,  "event_not_found",    self.to_string()),
            Self::CalendarNotFound(_) => (StatusCode::NOT_FOUND,  "calendar_not_found", self.to_string()),
            Self::InvalidICal(_)      => (StatusCode::BAD_REQUEST, "invalid_ical",       self.to_string()),
            Self::Conflict            => (StatusCode::CONFLICT,    "scheduling_conflict", self.to_string()),
            Self::Forbidden           => (StatusCode::FORBIDDEN,   "forbidden",          self.to_string()),
            Self::Core(_)             => (StatusCode::INTERNAL_SERVER_ERROR, "internal", "erro interno".into()),
        };
        (status, Json(json!({"error": code, "message": msg}))).into_response()
    }
}

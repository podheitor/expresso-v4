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

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("forbidden")]
    Forbidden,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("not supported: {0}")]
    NotSupported(&'static str),

    #[error("alarm not found: {0}")]
    AlarmNotFound(Uuid),
}

impl IntoResponse for CalendarError {
    fn into_response(self) -> Response {
        let (status, code, msg) = match &self {
            Self::EventNotFound(_)     => (StatusCode::NOT_FOUND,             "event_not_found",     self.to_string()),
            Self::CalendarNotFound(_)  => (StatusCode::NOT_FOUND,             "calendar_not_found",  self.to_string()),
            Self::InvalidICal(_)       => (StatusCode::BAD_REQUEST,           "invalid_ical",        self.to_string()),
            Self::BadRequest(_)        => (StatusCode::BAD_REQUEST,           "bad_request",         self.to_string()),
            Self::Conflict(_)          => (StatusCode::CONFLICT,              "conflict",            self.to_string()),
            Self::Forbidden            => (StatusCode::FORBIDDEN,             "forbidden",           self.to_string()),
            Self::DatabaseUnavailable  => (StatusCode::SERVICE_UNAVAILABLE,   "db_unavailable",      self.to_string()),
            Self::NotSupported(_)      => (StatusCode::NOT_IMPLEMENTED,       "not_supported",       self.to_string()),
            Self::AlarmNotFound(_)     => (StatusCode::NOT_FOUND,             "alarm_not_found",     self.to_string()),
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
#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    fn status(e: CalendarError) -> u16 {
        e.into_response().status().as_u16()
    }

    #[test]
    fn event_not_found_is_404() {
        assert_eq!(status(CalendarError::EventNotFound(Uuid::new_v4())), 404);
    }

    #[test]
    fn calendar_not_found_is_404() {
        assert_eq!(status(CalendarError::CalendarNotFound("default".into())), 404);
    }

    #[test]
    fn invalid_ical_is_400() {
        assert_eq!(status(CalendarError::InvalidICal("parse err".into())), 400);
    }

    #[test]
    fn bad_request_is_400() {
        assert_eq!(status(CalendarError::BadRequest("missing uid".into())), 400);
    }

    #[test]
    fn conflict_is_409() {
        assert_eq!(status(CalendarError::Conflict("uid dup".into())), 409);
    }

    #[test]
    fn forbidden_is_403() {
        assert_eq!(status(CalendarError::Forbidden), 403);
    }

    #[test]
    fn database_unavailable_is_503() {
        assert_eq!(status(CalendarError::DatabaseUnavailable), 503);
    }

    #[test]
    fn not_supported_is_501() {
        assert_eq!(status(CalendarError::NotSupported("MKCALENDAR")), 501);
    }

    #[test]
    fn alarm_not_found_is_404() {
        assert_eq!(status(CalendarError::AlarmNotFound(Uuid::new_v4())), 404);
    }

    #[test]
    fn calendar_forbidden_status_is_403() {
        assert_eq!(status(CalendarError::Forbidden), 403);
    }

    #[test]
    fn calendar_not_found_display_contains_name() {
        let e = CalendarError::CalendarNotFound("personal".into());
        assert!(format!("{e}").contains("personal"));
    }

    #[test]
    fn calendar_conflict_status_is_409() {
        assert_eq!(status(CalendarError::Conflict("uid dup".into())), 409);
    }

    #[test]
    fn forbidden_status_is_403() {
        assert_eq!(status(CalendarError::Forbidden), 403);
    }

    #[test]
    fn alarm_not_found_status_is_404() {
        use uuid::Uuid;
        assert_eq!(status(CalendarError::AlarmNotFound(Uuid::nil())), 404);
    }

    #[test]
    fn database_unavailable_display_is_nonempty() {
        assert!(!format!("{}", CalendarError::DatabaseUnavailable).is_empty());
    }

    #[test]
    fn bad_request_display_contains_message() {
        let e = CalendarError::BadRequest("no uid".into());
        assert!(format!("{e}").contains("no uid"));
    }

    #[test]
    fn invalid_ical_display_contains_message() {
        let e = CalendarError::InvalidICal("unexpected END".into());
        assert!(format!("{e}").contains("unexpected END"));
    }

    #[test]
    fn conflict_display_contains_message() {
        let e = CalendarError::Conflict("uid already exists".into());
        assert!(format!("{e}").contains("uid already exists"));
    }

    #[test]
    fn not_supported_status_is_501() {
        assert_eq!(status(CalendarError::NotSupported("REPORT")), 501);
    }

    #[test]
    fn event_not_found_display_contains_uuid() {
        let id = uuid::Uuid::nil();
        let e = CalendarError::EventNotFound(id);
        assert!(format!("{e}").contains(&id.to_string()));
    }
}

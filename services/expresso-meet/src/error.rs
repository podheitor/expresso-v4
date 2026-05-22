//! expresso-meet error types.

use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

pub type Result<T, E = MeetError> = std::result::Result<T, E>;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum MeetError {
    #[error("core: {0}")]
    Core(#[from] expresso_core::CoreError),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("database unavailable")]
    DatabaseUnavailable,

    #[error("jitsi not configured")]
    JitsiUnavailable,

    #[error("jwt error: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),

    #[error("meeting not found: {0}")]
    MeetingNotFound(Uuid),

    #[error("not a participant of this meeting")]
    NotParticipant,

    #[error("forbidden")]
    Forbidden,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("not found")]
    NotFound,
}

impl IntoResponse for MeetError {
    fn into_response(self) -> Response {
        let (status, code, msg) = match &self {
            Self::MeetingNotFound(_)  => (StatusCode::NOT_FOUND,           "meeting_not_found", self.to_string()),
            Self::NotFound            => (StatusCode::NOT_FOUND,           "not_found",         self.to_string()),
            Self::BadRequest(_)       => (StatusCode::BAD_REQUEST,         "bad_request",       self.to_string()),
            Self::Conflict(_)         => (StatusCode::CONFLICT,            "conflict",          self.to_string()),
            Self::NotParticipant      => (StatusCode::FORBIDDEN,           "not_participant",   self.to_string()),
            Self::Forbidden           => (StatusCode::FORBIDDEN,           "forbidden",         self.to_string()),
            Self::DatabaseUnavailable => (StatusCode::SERVICE_UNAVAILABLE, "db_unavailable",    self.to_string()),
            Self::JitsiUnavailable    => (StatusCode::SERVICE_UNAVAILABLE, "jitsi_unavailable", self.to_string()),
            Self::Jwt(_)              => (StatusCode::INTERNAL_SERVER_ERROR,"jwt",              "erro interno".into()),
            Self::Database(sqlx::Error::Database(db)) if db.is_unique_violation()
                                      => (StatusCode::CONFLICT,            "unique_violation",  "recurso duplicado".into()),
            Self::Database(sqlx::Error::RowNotFound)
                                      => (StatusCode::NOT_FOUND,           "not_found",         "recurso não encontrado".into()),
            Self::Database(_)         => (StatusCode::INTERNAL_SERVER_ERROR,"database",         "erro interno".into()),
            Self::Core(_)             => (StatusCode::INTERNAL_SERVER_ERROR,"internal",         "erro interno".into()),
        };
        (status, Json(json!({"error": code, "message": msg}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    fn status(e: MeetError) -> u16 {
        e.into_response().status().as_u16()
    }

    #[test]
    fn meeting_not_found_is_404() {
        assert_eq!(status(MeetError::MeetingNotFound(Uuid::new_v4())), 404);
    }

    #[test]
    fn not_found_is_404() {
        assert_eq!(status(MeetError::NotFound), 404);
    }

    #[test]
    fn bad_request_is_400() {
        assert_eq!(status(MeetError::BadRequest("bad".into())), 400);
    }

    #[test]
    fn conflict_is_409() {
        assert_eq!(status(MeetError::Conflict("dup".into())), 409);
    }

    #[test]
    fn not_participant_is_403() {
        assert_eq!(status(MeetError::NotParticipant), 403);
    }

    #[test]
    fn forbidden_is_403() {
        assert_eq!(status(MeetError::Forbidden), 403);
    }

    #[test]
    fn database_unavailable_is_503() {
        assert_eq!(status(MeetError::DatabaseUnavailable), 503);
    }

    #[test]
    fn jitsi_unavailable_is_503() {
        assert_eq!(status(MeetError::JitsiUnavailable), 503);
    }
}

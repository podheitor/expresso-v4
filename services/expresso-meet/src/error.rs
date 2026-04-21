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
}

impl IntoResponse for MeetError {
    fn into_response(self) -> Response {
        let (status, code, msg) = match &self {
            Self::MeetingNotFound(_)  => (StatusCode::NOT_FOUND,           "meeting_not_found", self.to_string()),
            Self::BadRequest(_)       => (StatusCode::BAD_REQUEST,         "bad_request",       self.to_string()),
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

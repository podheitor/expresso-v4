//! expresso-notes error types.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

pub type Result<T, E = NotesError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum NotesError {
    #[error("core: {0}")]
    Core(#[from] expresso_core::CoreError),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("database unavailable")]
    DatabaseUnavailable,

    #[error("note not found: {0}")]
    NoteNotFound(Uuid),

    #[error("notebook not found: {0}")]
    NotebookNotFound(Uuid),

    #[error("forbidden")]
    Forbidden,

    #[error("bad request: {0}")]
    BadRequest(String),
}

impl IntoResponse for NotesError {
    fn into_response(self) -> Response {
        let (status, code, msg) = match &self {
            Self::NoteNotFound(_) => (StatusCode::NOT_FOUND, "note_not_found", self.to_string()),
            Self::NotebookNotFound(_) => (
                StatusCode::NOT_FOUND,
                "notebook_not_found",
                self.to_string(),
            ),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden", self.to_string()),
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request", self.to_string()),
            Self::DatabaseUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "db_unavailable",
                self.to_string(),
            ),
            Self::Database(sqlx::Error::RowNotFound) => (
                StatusCode::NOT_FOUND,
                "not_found",
                "recurso não encontrado".into(),
            ),
            Self::Database(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "database",
                "erro interno".into(),
            ),
            Self::Core(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "erro interno".into(),
            ),
        };
        (status, Json(json!({"error": code, "message": msg}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(e: NotesError) -> StatusCode {
        e.into_response().status()
    }

    #[test]
    fn note_not_found_is_404() {
        assert_eq!(status(NotesError::NoteNotFound(Uuid::nil())), 404);
    }

    #[test]
    fn bad_request_is_400() {
        assert_eq!(status(NotesError::BadRequest("x".into())), 400);
    }

    #[test]
    fn db_unavailable_is_503() {
        assert_eq!(status(NotesError::DatabaseUnavailable), 503);
    }
}

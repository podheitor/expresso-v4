//! expresso-chat error types.

use axum::{response::{IntoResponse, Response}, http::StatusCode, Json};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

pub type Result<T, E = ChatError> = std::result::Result<T, E>;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum ChatError {
    #[error("core: {0}")]
    Core(#[from] expresso_core::CoreError),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("database unavailable")]
    DatabaseUnavailable,

    #[error("matrix backend unavailable")]
    MatrixUnavailable,

    #[error("matrix error: {0}")]
    Matrix(String),

    #[error("channel not found: {0}")]
    ChannelNotFound(Uuid),

    #[error("not a member of this channel")]
    NotMember,

    #[error("forbidden")]
    Forbidden,

    #[error("bad request: {0}")]
    BadRequest(String),
}

impl IntoResponse for ChatError {
    fn into_response(self) -> Response {
        let (status, code, msg) = match &self {
            Self::ChannelNotFound(_)  => (StatusCode::NOT_FOUND,           "channel_not_found", self.to_string()),
            Self::BadRequest(_)       => (StatusCode::BAD_REQUEST,         "bad_request",       self.to_string()),
            Self::NotMember           => (StatusCode::FORBIDDEN,           "not_member",        self.to_string()),
            Self::Forbidden           => (StatusCode::FORBIDDEN,           "forbidden",         self.to_string()),
            Self::DatabaseUnavailable => (StatusCode::SERVICE_UNAVAILABLE, "db_unavailable",    self.to_string()),
            Self::MatrixUnavailable   => (StatusCode::SERVICE_UNAVAILABLE, "matrix_unavailable",self.to_string()),
            Self::Matrix(_)           => (StatusCode::BAD_GATEWAY,         "matrix_backend",    self.to_string()),
            Self::Database(sqlx::Error::Database(db_err)) if db_err.is_unique_violation()
                                      => (StatusCode::CONFLICT,            "unique_violation",  "recurso duplicado".into()),
            Self::Database(sqlx::Error::RowNotFound)
                                      => (StatusCode::NOT_FOUND,           "not_found",         "recurso não encontrado".into()),
            Self::Database(_)         => (StatusCode::INTERNAL_SERVER_ERROR,"database",         "erro interno".into()),
            Self::Core(_)             => (StatusCode::INTERNAL_SERVER_ERROR,"internal",         "erro interno".into()),
        };
        (status, Json(json!({"error": code, "message": msg}))).into_response()
    }
}

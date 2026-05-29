//! expresso-chat error types.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
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
            Self::ChannelNotFound(_) => {
                (StatusCode::NOT_FOUND, "channel_not_found", self.to_string())
            }
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request", self.to_string()),
            Self::NotMember => (StatusCode::FORBIDDEN, "not_member", self.to_string()),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden", self.to_string()),
            Self::DatabaseUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "db_unavailable",
                self.to_string(),
            ),
            Self::MatrixUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "matrix_unavailable",
                self.to_string(),
            ),
            Self::Matrix(_) => (StatusCode::BAD_GATEWAY, "matrix_backend", self.to_string()),
            Self::Database(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => (
                StatusCode::CONFLICT,
                "unique_violation",
                "recurso duplicado".into(),
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
    use axum::response::IntoResponse;

    fn status(e: ChatError) -> u16 {
        e.into_response().status().as_u16()
    }

    #[test]
    fn channel_not_found_is_404() {
        assert_eq!(status(ChatError::ChannelNotFound(Uuid::new_v4())), 404);
    }

    #[test]
    fn bad_request_is_400() {
        assert_eq!(status(ChatError::BadRequest("bad".into())), 400);
    }

    #[test]
    fn not_member_is_403() {
        assert_eq!(status(ChatError::NotMember), 403);
    }

    #[test]
    fn forbidden_is_403() {
        assert_eq!(status(ChatError::Forbidden), 403);
    }

    #[test]
    fn database_unavailable_is_503() {
        assert_eq!(status(ChatError::DatabaseUnavailable), 503);
    }

    #[test]
    fn matrix_unavailable_is_503() {
        assert_eq!(status(ChatError::MatrixUnavailable), 503);
    }

    #[test]
    fn matrix_error_is_502() {
        assert_eq!(status(ChatError::Matrix("upstream".into())), 502);
    }

    #[test]
    fn bad_request_message_preserved() {
        let e = ChatError::BadRequest("invalid channel name".into());
        let msg = format!("{e}");
        assert!(msg.contains("invalid channel name"));
    }

    #[test]
    fn channel_not_found_display_contains_id() {
        let id = Uuid::nil();
        let e = ChatError::ChannelNotFound(id);
        assert!(format!("{e}").contains(&id.to_string()));
    }

    #[test]
    fn not_member_display_not_empty() {
        let msg = format!("{}", ChatError::NotMember);
        assert!(!msg.is_empty());
    }

    #[test]
    fn forbidden_display_not_empty() {
        let msg = format!("{}", ChatError::Forbidden);
        assert!(!msg.is_empty());
    }

    #[test]
    fn matrix_error_status_is_502() {
        assert_eq!(status(ChatError::Matrix("upstream timeout".into())), 502);
    }

    #[test]
    fn matrix_unavailable_display_not_empty() {
        let msg = format!("{}", ChatError::MatrixUnavailable);
        assert!(!msg.is_empty());
    }

    #[test]
    fn bad_request_status_is_400() {
        assert_eq!(status(ChatError::BadRequest("x".into())), 400);
    }

    #[test]
    fn internal_status_is_500() {
        // No `Internal` variant; Core(_) is the internal-error path (→ 500).
        assert_eq!(
            status(ChatError::Core(expresso_core::CoreError::NotFound {
                resource: "x"
            })),
            500
        );
    }

    #[test]
    fn matrix_display_contains_message() {
        let e = ChatError::Matrix("timeout after 5s".into());
        assert!(format!("{e}").contains("timeout after 5s"));
    }

    #[test]
    fn database_unavailable_status_is_503() {
        assert_eq!(status(ChatError::DatabaseUnavailable), 503);
    }

    #[test]
    fn channel_not_found_status_is_404() {
        assert_eq!(status(ChatError::ChannelNotFound(Uuid::nil())), 404);
    }

    #[test]
    fn message_carrying_error_display_contains_message() {
        // `Internal` was removed; Matrix(String) carries a backend message.
        let e = ChatError::Matrix("disk full".into());
        assert!(format!("{e}").contains("disk full"));
    }

    #[test]
    fn not_member_status_is_403() {
        assert_eq!(status(ChatError::NotMember), 403);
    }

    #[test]
    fn matrix_error_display_contains_message() {
        let e = ChatError::Matrix("timeout".into());
        assert!(format!("{e}").contains("timeout"));
    }

    #[test]
    fn forbidden_display_is_forbidden() {
        let e = ChatError::Forbidden;
        assert!(format!("{e}").contains("forbidden"));
    }

    #[test]
    fn matrix_unavailable_status_is_503() {
        assert_eq!(status(ChatError::MatrixUnavailable), 503);
    }

    #[test]
    fn bad_request_display_contains_message() {
        let e = ChatError::BadRequest("empty body".into());
        assert!(format!("{e}").contains("empty body"));
    }

    #[test]
    fn channel_not_found_display_is_nonempty() {
        let e = ChatError::ChannelNotFound(uuid::Uuid::nil());
        assert!(!format!("{e}").is_empty());
    }
}

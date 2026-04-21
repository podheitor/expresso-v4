use axum::{response::{IntoResponse, Response}, http::StatusCode, Json};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum MailError {
    #[error(transparent)]
    Core(#[from] expresso_core::CoreError),

    #[error("message not found: {0}")]
    MessageNotFound(uuid::Uuid),

    #[error("folder not found: {folder}")]
    FolderNotFound { folder: String },

    #[error("quota exceeded")]
    QuotaExceeded,

    #[error("send failed: {0}")]
    SendFailed(String),

    #[error("SMTP protocol error: {0}")]
    SmtpProtocol(String),

    #[error("invalid message format: {0}")]
    InvalidMessage(String),

    #[error("forbidden")]
    Forbidden,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

pub type Result<T> = std::result::Result<T, MailError>;

impl IntoResponse for MailError {
    fn into_response(self) -> Response {
        let (status, code, msg) = match &self {
            MailError::MessageNotFound(_) | MailError::FolderNotFound { .. } => {
                (StatusCode::NOT_FOUND, "not_found", self.to_string())
            }
            MailError::QuotaExceeded => {
                (StatusCode::PAYLOAD_TOO_LARGE, "quota_exceeded", self.to_string())
            }
            MailError::Forbidden => {
                (StatusCode::FORBIDDEN, "forbidden", self.to_string())
            }
            MailError::InvalidMessage(m) => {
                (StatusCode::BAD_REQUEST, "invalid_message", m.clone())
            }
            MailError::BadRequest(m) => {
                (StatusCode::BAD_REQUEST, "bad_request", m.clone())
            }
            _ => {
                tracing::error!(error = %self, "internal mail error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "internal server error".into())
            }
        };

        (status, Json(json!({"error": code, "message": msg}))).into_response()
    }
}

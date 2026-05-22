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

    #[error("not found")]
    NotFound,

    #[error("forbidden")]
    Forbidden,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

pub type Result<T> = std::result::Result<T, MailError>;

impl IntoResponse for MailError {
    fn into_response(self) -> Response {
        let (status, code, msg) = match &self {
            MailError::MessageNotFound(_) | MailError::FolderNotFound { .. } | MailError::NotFound => {
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
            MailError::Conflict(m) => {
                (StatusCode::CONFLICT, "conflict", m.clone())
            }
            _ => {
                tracing::error!(error = %self, "internal mail error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "internal server error".into())
            }
        };

        (status, Json(json!({"error": code, "message": msg}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    fn status(e: MailError) -> u16 {
        e.into_response().status().as_u16()
    }

    #[test]
    fn message_not_found_is_404() {
        assert_eq!(status(MailError::MessageNotFound(uuid::Uuid::new_v4())), 404);
    }

    #[test]
    fn folder_not_found_is_404() {
        assert_eq!(status(MailError::FolderNotFound { folder: "INBOX".into() }), 404);
    }

    #[test]
    fn not_found_is_404() {
        assert_eq!(status(MailError::NotFound), 404);
    }

    #[test]
    fn quota_exceeded_is_413() {
        assert_eq!(status(MailError::QuotaExceeded), 413);
    }

    #[test]
    fn forbidden_is_403() {
        assert_eq!(status(MailError::Forbidden), 403);
    }

    #[test]
    fn invalid_message_is_400() {
        assert_eq!(status(MailError::InvalidMessage("bad mime".into())), 400);
    }

    #[test]
    fn bad_request_is_400() {
        assert_eq!(status(MailError::BadRequest("missing field".into())), 400);
    }

    #[test]
    fn conflict_is_409() {
        assert_eq!(status(MailError::Conflict("uid clash".into())), 409);
    }

    #[test]
    fn send_failed_is_500() {
        assert_eq!(status(MailError::SendFailed("relay down".into())), 500);
    }

    #[test]
    fn smtp_protocol_is_500() {
        assert_eq!(status(MailError::SmtpProtocol("421".into())), 500);
    }

    #[test]
    fn message_not_found_display_contains_id() {
        let id = uuid::Uuid::nil();
        let e = MailError::MessageNotFound(id);
        assert!(format!("{e}").contains(&id.to_string()));
    }

    #[test]
    fn bad_request_display_contains_message() {
        let e = MailError::BadRequest("missing from".into());
        assert!(format!("{e}").contains("missing from"));
    }

    #[test]
    fn forbidden_status_is_403() {
        assert_eq!(status(MailError::Forbidden), 403);
    }

    #[test]
    fn quota_exceeded_status_is_507() {
        assert_eq!(status(MailError::QuotaExceeded), 507);
    }

    #[test]
    fn send_failed_display_contains_message() {
        let e = MailError::SendFailed("relay refused".into());
        assert!(format!("{e}").contains("relay refused"));
    }

    #[test]
    fn conflict_display_contains_message() {
        let e = MailError::Conflict("duplicate message-id".into());
        assert!(format!("{e}").contains("duplicate message-id"));
    }

    #[test]
    fn smtp_protocol_display_contains_code() {
        let e = MailError::SmtpProtocol("550".into());
        assert!(format!("{e}").contains("550"));
    }

    #[test]
    fn invalid_message_is_400_status() {
        assert_eq!(status(MailError::InvalidMessage("bad mime".into())), 400);
    }

    #[test]
    fn quota_exceeded_display_not_empty() {
        let e = MailError::QuotaExceeded;
        assert!(!format!("{e}").is_empty());
    }
}

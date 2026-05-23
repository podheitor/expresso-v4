//! Error taxonomy for expresso-notifications.

use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("subscriber not found")] NotFound,
    #[error("invalid payload: {0}")] BadRequest(String),
    #[error("storage error: {0}")] Storage(String),
    #[error("dispatch error: {0}")] Dispatch(String),
    #[error("internal: {0}")] Internal(String),
}

impl IntoResponse for NotifyError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            NotifyError::NotFound       => (StatusCode::NOT_FOUND,            "not_found"),
            NotifyError::BadRequest(_)  => (StatusCode::BAD_REQUEST,          "bad_request"),
            NotifyError::Storage(_)     => (StatusCode::INTERNAL_SERVER_ERROR,"storage_error"),
            NotifyError::Dispatch(_)    => (StatusCode::BAD_GATEWAY,          "dispatch_error"),
            NotifyError::Internal(_)    => (StatusCode::INTERNAL_SERVER_ERROR,"internal"),
        };
        let body = Json(json!({"error": code, "message": self.to_string()}));
        (status, body).into_response()
    }
}

pub type NotifyResult<T> = Result<T, NotifyError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    fn status(e: NotifyError) -> StatusCode {
        e.into_response().status()
    }

    #[test]
    fn not_found_is_404() {
        assert_eq!(status(NotifyError::NotFound), StatusCode::NOT_FOUND);
    }

    #[test]
    fn bad_request_is_400() {
        assert_eq!(status(NotifyError::BadRequest("bad".into())), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn storage_error_is_500() {
        assert_eq!(status(NotifyError::Storage("db fail".into())), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn dispatch_error_is_502() {
        assert_eq!(status(NotifyError::Dispatch("push fail".into())), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn internal_is_500() {
        assert_eq!(status(NotifyError::Internal("oops".into())), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn not_found_display_contains_subscriber() {
        let e = NotifyError::NotFound;
        assert!(e.to_string().contains("not found"));
    }

    #[test]
    fn bad_request_display_contains_message() {
        let e = NotifyError::BadRequest("missing field".into());
        assert!(e.to_string().contains("missing field"));
    }

    #[test]
    fn storage_error_display_contains_message() {
        let e = NotifyError::Storage("timeout".into());
        assert!(e.to_string().contains("timeout"));
    }

    #[test]
    fn dispatch_error_display_contains_message() {
        let e = NotifyError::Dispatch("vapid failed".into());
        assert!(e.to_string().contains("vapid failed"));
    }

    #[test]
    fn internal_display_contains_message() {
        let e = NotifyError::Internal("null ptr".into());
        assert!(e.to_string().contains("null ptr"));
    }

    #[test]
    fn not_found_is_not_found_variant() {
        assert!(matches!(NotifyError::NotFound, NotifyError::NotFound));
    }

    #[test]
    fn bad_request_is_bad_request_variant() {
        assert!(matches!(NotifyError::BadRequest(_), NotifyError::BadRequest(_)));
    }

    #[test]
    fn storage_is_storage_variant() {
        assert!(matches!(NotifyError::Storage("x".into()), NotifyError::Storage(_)));
    }

    #[test]
    fn dispatch_is_dispatch_variant() {
        assert!(matches!(NotifyError::Dispatch("x".into()), NotifyError::Dispatch(_)));
    }

    #[test]
    fn internal_is_internal_variant() {
        assert!(matches!(NotifyError::Internal("x".into()), NotifyError::Internal(_)));
    }

    #[test]
    fn not_found_display_nonempty() {
        assert!(!NotifyError::NotFound.to_string().is_empty());
    }

    #[test]
    fn bad_request_display_starts_with_invalid() {
        let e = NotifyError::BadRequest("x".into());
        assert!(e.to_string().starts_with("invalid payload:"));
    }

    #[test]
    fn storage_display_starts_with_storage() {
        let e = NotifyError::Storage("x".into());
        assert!(e.to_string().starts_with("storage error:"));
    }

    #[test]
    fn dispatch_display_starts_with_dispatch() {
        let e = NotifyError::Dispatch("x".into());
        assert!(e.to_string().starts_with("dispatch error:"));
    }

    #[test]
    fn internal_display_starts_with_internal() {
        let e = NotifyError::Internal("x".into());
        assert!(e.to_string().starts_with("internal:"));
    }

    #[test]
    fn not_found_status_is_404_via_response() {
        let resp = NotifyError::NotFound.into_response();
        assert_eq!(resp.status().as_u16(), 404);
    }

    #[test]
    fn dispatch_status_as_u16_is_502() {
        let resp = NotifyError::Dispatch("err".into()).into_response();
        assert_eq!(resp.status().as_u16(), 502);
    }

    #[test]
    fn bad_request_and_not_found_differ() {
        assert_ne!(
            format!("{}", NotifyError::BadRequest("x".into())),
            format!("{}", NotifyError::NotFound)
        );
    }

    #[test]
    fn notify_result_ok_is_ok() {
        let r: NotifyResult<u32> = Ok(42);
        assert!(r.is_ok());
    }

    #[test]
    fn notify_result_err_is_err() {
        let r: NotifyResult<u32> = Err(NotifyError::NotFound);
        assert!(r.is_err());
    }
}

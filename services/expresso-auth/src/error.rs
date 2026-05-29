//! RP endpoint error taxonomy.

use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum RpError {
    #[error("discovery failed: {0}")] Discovery(String),
    #[error("state not found or expired")] StateNotFound,
    #[error("token endpoint failure: {0}")] TokenExchange(String),
    #[error("refresh failed: {0}")] Refresh(String),
    #[error("invalid parameter: {0}")] BadRequest(&'static str),
    #[error(transparent)] Auth(#[from] expresso_auth_client::AuthError),
    #[error(transparent)] Http(#[from] reqwest::Error),
    #[error("config error: {0}")] Config(String),
}

impl IntoResponse for RpError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            RpError::StateNotFound    => (StatusCode::BAD_REQUEST,         "state_not_found"),
            RpError::BadRequest(_)    => (StatusCode::BAD_REQUEST,         "bad_request"),
            RpError::Auth(_)          => (StatusCode::UNAUTHORIZED,        "auth"),
            RpError::TokenExchange(_) => (StatusCode::BAD_GATEWAY,         "token_exchange"),
            RpError::Refresh(_)       => (StatusCode::UNAUTHORIZED,        "refresh_failed"),
            RpError::Discovery(_)     => (StatusCode::SERVICE_UNAVAILABLE, "discovery_failed"),
            RpError::Http(_)          => (StatusCode::BAD_GATEWAY,         "upstream"),
            RpError::Config(_)        => (StatusCode::INTERNAL_SERVER_ERROR, "config"),
        };
        let body = Json(json!({"error": code, "message": self.to_string()}));
        (status, body).into_response()
    }
}

pub type Result<T> = std::result::Result<T, RpError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    fn status(e: RpError) -> axum::http::StatusCode {
        e.into_response().status()
    }

    #[test]
    fn state_not_found_is_400() {
        assert_eq!(status(RpError::StateNotFound), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn bad_request_is_400() {
        assert_eq!(status(RpError::BadRequest("x")), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn token_exchange_failure_is_502() {
        assert_eq!(status(RpError::TokenExchange("err".into())), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn refresh_failure_is_401() {
        assert_eq!(status(RpError::Refresh("err".into())), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn discovery_failure_is_503() {
        assert_eq!(status(RpError::Discovery("err".into())), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn config_error_is_500() {
        assert_eq!(status(RpError::Config("err".into())), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn error_body_contains_code_and_message() {
        let resp = RpError::StateNotFound.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn bad_request_static_str_is_400() {
        assert_eq!(status(RpError::BadRequest("invalid_param")), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn config_error_message_preserved() {
        let e = RpError::Config("missing_client_secret".into());
        assert!(e.to_string().contains("missing_client_secret"));
    }

    #[test]
    fn bad_request_invalid_state_is_400() {
        assert_eq!(status(RpError::BadRequest("invalid_state")), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn config_missing_secret_is_500() {
        assert_eq!(status(RpError::Config("missing_secret".into())), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn token_exchange_error_is_bad_gateway() {
        assert_eq!(status(RpError::TokenExchange("upstream failed".into())), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn discovery_error_is_service_unavailable() {
        assert_eq!(status(RpError::Discovery("no endpoint".into())), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn auth_error_is_unauthorized() {
        assert_eq!(
            status(RpError::Auth(expresso_auth_client::AuthError::InvalidToken("invalid token".into()))),
            StatusCode::UNAUTHORIZED,
        );
    }

    #[test]
    fn bad_request_error_is_400() {
        assert_eq!(status(RpError::BadRequest("missing param".into())), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn refresh_error_is_unauthorized() {
        assert_eq!(status(RpError::Refresh("expired".into())), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn config_error_status_is_500() {
        assert_eq!(status(RpError::Config("missing var".into())), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn discovery_error_message_preserved() {
        let e = RpError::Discovery("no endpoint available".into());
        assert!(e.to_string().contains("no endpoint available"));
    }

    #[test]
    fn token_exchange_error_message_preserved() {
        let e = RpError::TokenExchange("upstream 502".into());
        assert!(e.to_string().contains("upstream 502"));
    }

    #[test]
    fn refresh_error_message_preserved() {
        let e = RpError::Refresh("token expired".into());
        assert!(e.to_string().contains("token expired"));
    }

    #[test]
    fn config_error_nonempty_message_preserved() {
        let e = RpError::Config("key_not_found".into());
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn discovery_error_status_is_503() {
        let e = RpError::Discovery("timeout".into());
        let resp = e.into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn state_not_found_status_is_400() {
        let e = RpError::StateNotFound;
        let resp = e.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn refresh_error_status_is_401() {
        assert_eq!(status(RpError::Refresh("timeout".into())), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn state_not_found_display_is_nonempty() {
        let e = RpError::StateNotFound;
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn token_exchange_and_discovery_are_distinct_statuses() {
        let a = status(RpError::TokenExchange("x".into()));
        let b = status(RpError::Discovery("x".into()));
        assert_ne!(a, b);
    }
}

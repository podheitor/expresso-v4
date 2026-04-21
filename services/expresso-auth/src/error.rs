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
        };
        let body = Json(json!({"error": code, "message": self.to_string()}));
        (status, body).into_response()
    }
}

pub type Result<T> = std::result::Result<T, RpError>;

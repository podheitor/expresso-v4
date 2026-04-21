//! Error type for expresso-web. Always render HTML error page on failure.

use axum::{http::StatusCode, response::{IntoResponse, Response}};

#[derive(Debug, thiserror::Error)]
pub enum WebError {
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error("internal: {0}")]
    Internal(String),
}

impl From<reqwest::Error> for WebError {
    fn from(e: reqwest::Error) -> Self { WebError::Upstream(e.to_string()) }
}
impl From<serde_json::Error> for WebError {
    fn from(e: serde_json::Error) -> Self { WebError::Internal(e.to_string()) }
}
impl From<anyhow::Error> for WebError {
    fn from(e: anyhow::Error) -> Self { WebError::Internal(e.to_string()) }
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let body = format!(
            "<!doctype html><meta charset=utf-8><title>Erro</title>\
             <body style=\"font-family:system-ui;padding:2rem\">\
             <h1>Erro interno</h1><pre>{}</pre>\
             <p><a href=\"/\">Voltar</a></p></body>",
            html_escape::encode_text(&self.to_string())
        );
        (StatusCode::INTERNAL_SERVER_ERROR,
         [("content-type", "text/html; charset=utf-8")],
         body).into_response()
    }
}

pub type WebResult<T> = Result<T, WebError>;

// expresso-core is required by thiserror import. re-export placeholders below.
// (thiserror is workspace dep)

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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn upstream_is_500() {
        let e = WebError::Upstream("connect refused".into());
        assert_eq!(e.into_response().status().as_u16(), 500);
    }

    #[test]
    fn internal_is_500() {
        let e = WebError::Internal("template panic".into());
        assert_eq!(e.into_response().status().as_u16(), 500);
    }

    #[test]
    fn upstream_display_contains_message() {
        let e = WebError::Upstream("timeout".into());
        assert!(e.to_string().contains("timeout"));
    }

    #[test]
    fn internal_display_contains_message() {
        let e = WebError::Internal("nil ptr".into());
        assert!(e.to_string().contains("nil ptr"));
    }

    #[test]
    fn from_serde_json_error_is_internal() {
        let json_err = serde_json::from_str::<i32>("not_json").unwrap_err();
        let e: WebError = json_err.into();
        assert!(matches!(e, WebError::Internal(_)));
    }

    #[test]
    fn from_anyhow_is_internal() {
        let e: WebError = anyhow::anyhow!("anyhow reason").into();
        assert!(matches!(e, WebError::Internal(_)));
        assert!(e.to_string().contains("anyhow reason"));
    }

    #[test]
    fn response_content_type_is_html() {
        use axum::response::IntoResponse;
        let resp = WebError::Internal("x".into()).into_response();
        let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
        assert!(ct.contains("text/html"));
    }
}

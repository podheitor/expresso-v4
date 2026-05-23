//! Error taxonomy for expresso-search.

use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("document not found")] NotFound,
    #[error("bad query: {0}")] BadQuery(String),
    #[error("index error: {0}")] IndexError(String),
    #[error("invalid tenant: {0}")] InvalidTenant(String),
    #[error("internal: {0}")] Internal(String),
}

impl IntoResponse for SearchError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            SearchError::NotFound        => (StatusCode::NOT_FOUND,            "not_found"),
            SearchError::BadQuery(_)     => (StatusCode::BAD_REQUEST,          "bad_query"),
            SearchError::IndexError(_)   => (StatusCode::INTERNAL_SERVER_ERROR,"index_error"),
            SearchError::InvalidTenant(_)=> (StatusCode::BAD_REQUEST,          "invalid_tenant"),
            SearchError::Internal(_)     => (StatusCode::INTERNAL_SERVER_ERROR,"internal"),
        };
        let body = Json(json!({"error": code, "message": self.to_string()}));
        (status, body).into_response()
    }
}

pub type SearchResult<T> = Result<T, SearchError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    fn status(e: SearchError) -> StatusCode {
        e.into_response().status()
    }

    #[test]
    fn not_found_is_404() {
        assert_eq!(status(SearchError::NotFound), StatusCode::NOT_FOUND);
    }

    #[test]
    fn bad_query_is_400() {
        assert_eq!(status(SearchError::BadQuery("syntax error".into())), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn index_error_is_500() {
        assert_eq!(status(SearchError::IndexError("lock fail".into())), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn invalid_tenant_is_400() {
        assert_eq!(status(SearchError::InvalidTenant("not-uuid".into())), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn internal_is_500() {
        assert_eq!(status(SearchError::Internal("crash".into())), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn not_found_display_contains_not_found() {
        assert!(SearchError::NotFound.to_string().contains("not found"));
    }

    #[test]
    fn bad_query_display_contains_message() {
        let e = SearchError::BadQuery("unbalanced parens".into());
        assert!(e.to_string().contains("unbalanced parens"));
    }

    #[test]
    fn index_error_display_contains_message() {
        let e = SearchError::IndexError("writer busy".into());
        assert!(e.to_string().contains("writer busy"));
    }

    #[test]
    fn invalid_tenant_display_contains_message() {
        let e = SearchError::InvalidTenant("wildcard".into());
        assert!(e.to_string().contains("wildcard"));
    }

    #[test]
    fn internal_display_contains_message() {
        let e = SearchError::Internal("nil deref".into());
        assert!(e.to_string().contains("nil deref"));
    }

    #[test]
    fn not_found_is_not_found_variant() {
        assert!(matches!(SearchError::NotFound, SearchError::NotFound));
    }

    #[test]
    fn bad_query_display_starts_with_bad_query() {
        let e = SearchError::BadQuery("x".into());
        assert!(e.to_string().starts_with("bad query:"));
    }

    #[test]
    fn index_error_display_starts_with_index_error() {
        let e = SearchError::IndexError("x".into());
        assert!(e.to_string().starts_with("index error:"));
    }

    #[test]
    fn invalid_tenant_display_starts_with_invalid() {
        let e = SearchError::InvalidTenant("x".into());
        assert!(e.to_string().starts_with("invalid tenant:"));
    }

    #[test]
    fn internal_display_starts_with_internal() {
        let e = SearchError::Internal("x".into());
        assert!(e.to_string().starts_with("internal:"));
    }

    #[test]
    fn not_found_status_as_u16_is_404() {
        let resp = SearchError::NotFound.into_response();
        assert_eq!(resp.status().as_u16(), 404);
    }

    #[test]
    fn bad_query_status_as_u16_is_400() {
        let resp = SearchError::BadQuery("x".into()).into_response();
        assert_eq!(resp.status().as_u16(), 400);
    }

    #[test]
    fn index_error_status_as_u16_is_500() {
        let resp = SearchError::IndexError("x".into()).into_response();
        assert_eq!(resp.status().as_u16(), 500);
    }

    #[test]
    fn internal_status_as_u16_is_500() {
        let resp = SearchError::Internal("x".into()).into_response();
        assert_eq!(resp.status().as_u16(), 500);
    }

    #[test]
    fn search_result_ok_is_ok() {
        let r: SearchResult<i32> = Ok(1);
        assert!(r.is_ok());
    }

    #[test]
    fn search_result_err_is_err() {
        let r: SearchResult<i32> = Err(SearchError::NotFound);
        assert!(r.is_err());
    }

    #[test]
    fn not_found_and_bad_query_display_differ() {
        assert_ne!(
            format!("{}", SearchError::NotFound),
            format!("{}", SearchError::BadQuery("x".into()))
        );
    }

    #[test]
    fn index_error_and_internal_display_differ() {
        assert_ne!(
            format!("{}", SearchError::IndexError("x".into())),
            format!("{}", SearchError::Internal("x".into()))
        );
    }

    #[test]
    fn search_result_err_carries_variant() {
        let r: SearchResult<i32> = Err(SearchError::BadQuery("oops".into()));
        assert!(matches!(r, Err(SearchError::BadQuery(_))));
    }
}

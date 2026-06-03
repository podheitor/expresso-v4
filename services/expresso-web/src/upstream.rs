//! Upstream HTTP client helpers — forward cookies + tenant/user context.

use crate::{
    error::{WebError, WebResult},
    AppState,
};
use axum::http::HeaderMap;
use bytes::Bytes;
use reqwest::{RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;

/// Copy `cookie` header from incoming request onto outgoing request.
pub fn fwd_cookie(mut req: RequestBuilder, headers: &HeaderMap) -> RequestBuilder {
    if let Some(c) = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
    {
        req = req.header("cookie", c);
    }
    req
}

/// Inject `x-tenant-id` + `x-user-id` headers from Me (when known).
pub fn inject_ctx(req: RequestBuilder, tenant_id: &str, user_id: &str) -> RequestBuilder {
    req.header("x-tenant-id", tenant_id)
        .header("x-user-id", user_id)
}

fn build_url(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

/// GET JSON → Some(T) on 2xx, None on 401/403, Err on other failures.
pub async fn get_json<T: DeserializeOwned>(
    state: &AppState,
    base: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: Option<(&str, &str)>,
) -> WebResult<Option<T>> {
    let url = build_url(base, path);
    let mut req = state.http.get(&url);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx {
        req = inject_ctx(req, t, u);
    }
    let resp = req.send().await?;
    match resp.status() {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Ok(None),
        s if s.is_success() => Ok(Some(resp.json::<T>().await?)),
        s => {
            let txt = resp.text().await.unwrap_or_default();
            Err(WebError::Upstream(format!("{} {}: {}", s, url, txt)))
        }
    }
}

/// POST no body → propaga status code upstream.
pub async fn post_empty(
    state: &AppState,
    base: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: Option<(&str, &str)>,
) -> WebResult<u16> {
    let url = build_url(base, path);
    let mut req = state.http.post(&url).header("content-length", "0");
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx {
        req = inject_ctx(req, t, u);
    }
    Ok(req.send().await?.status().as_u16())
}

/// DELETE → propaga status code.
pub async fn delete_at(
    state: &AppState,
    base: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: Option<(&str, &str)>,
) -> WebResult<u16> {
    let url = build_url(base, path);
    let mut req = state.http.delete(&url);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx {
        req = inject_ctx(req, t, u);
    }
    Ok(req.send().await?.status().as_u16())
}

/// DELETE com JSON body → propaga status. Alguns endpoints (ex.: remover
/// participante de breakout) carregam o alvo no corpo da requisição.
pub async fn delete_json<T: serde::Serialize>(
    state: &AppState,
    base: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: Option<(&str, &str)>,
    body: &T,
) -> WebResult<u16> {
    let url = build_url(base, path);
    let mut req = state.http.delete(&url).json(body);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx {
        req = inject_ctx(req, t, u);
    }
    Ok(req.send().await?.status().as_u16())
}

/// POST com body + content-type → propaga status.
/// Utilizado p/ repassar multipart/form-data do browser p/ backend.
pub async fn post_body(
    state: &AppState,
    base: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: Option<(&str, &str)>,
    body: Bytes,
    content_type: &str,
) -> WebResult<u16> {
    let url = build_url(base, path);
    let mut req = state
        .http
        .post(&url)
        .header("content-type", content_type)
        .body(body);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx {
        req = inject_ctx(req, t, u);
    }
    Ok(req.send().await?.status().as_u16())
}

/// POST com body + content-type → devolve `(status, body)`, onde `body` é o
/// JSON da resposta quando 2xx (e parseável), senão None. Para quando o caller
/// precisa do recurso criado (ex.: o `id` do evento p/ enfileirar alarmes).
pub async fn post_body_json(
    state: &AppState,
    base: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: Option<(&str, &str)>,
    body: Bytes,
    content_type: &str,
) -> WebResult<(u16, Option<serde_json::Value>)> {
    let url = build_url(base, path);
    let mut req = state
        .http
        .post(&url)
        .header("content-type", content_type)
        .body(body);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx {
        req = inject_ctx(req, t, u);
    }
    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let json = if (200..300).contains(&status) {
        resp.json::<serde_json::Value>().await.ok()
    } else {
        None
    };
    Ok((status, json))
}

/// POST com JSON body → propaga status.
pub async fn post_json<T: serde::Serialize>(
    state: &AppState,
    base: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: Option<(&str, &str)>,
    body: &T,
) -> WebResult<u16> {
    let url = build_url(base, path);
    let mut req = state.http.post(&url).json(body);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx {
        req = inject_ctx(req, t, u);
    }
    Ok(req.send().await?.status().as_u16())
}

/// POST com JSON body → devolve `(status, Option<Value>)`, com o JSON da
/// resposta quando 2xx (e parseável). Para quando o caller precisa do recurso
/// criado (ex.: id + deliver_at do undo-send).
pub async fn post_json_body<T: serde::Serialize>(
    state: &AppState,
    base: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: Option<(&str, &str)>,
    body: &T,
) -> WebResult<(u16, Option<serde_json::Value>)> {
    let url = build_url(base, path);
    let mut req = state.http.post(&url).json(body);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx {
        req = inject_ctx(req, t, u);
    }
    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let json = if (200..300).contains(&status) {
        resp.json::<serde_json::Value>().await.ok()
    } else {
        None
    };
    Ok((status, json))
}

/// PUT com JSON body → propaga status.
pub async fn put_json<T: serde::Serialize>(
    state: &AppState,
    base: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: Option<(&str, &str)>,
    body: &T,
) -> WebResult<u16> {
    let url = build_url(base, path);
    let mut req = state.http.put(&url).json(body);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx {
        req = inject_ctx(req, t, u);
    }
    Ok(req.send().await?.status().as_u16())
}

/// GET → proxy raw bytes + headers (content-type, content-disposition).
pub async fn get_bytes(
    state: &AppState,
    base: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: Option<(&str, &str)>,
) -> WebResult<(u16, Option<String>, Option<String>, Bytes)> {
    let url = build_url(base, path);
    let mut req = state.http.get(&url);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx {
        req = inject_ctx(req, t, u);
    }
    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let cd = resp
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let body = resp.bytes().await?;
    Ok((status, ct, cd, body))
}

/// PATCH com JSON body → propaga status.
pub async fn patch_json<T: serde::Serialize>(
    state: &AppState,
    base: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: Option<(&str, &str)>,
    body: &T,
) -> WebResult<u16> {
    let url = build_url(base, path);
    let mut req = state.http.patch(&url).json(body);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx {
        req = inject_ctx(req, t, u);
    }
    Ok(req.send().await?.status().as_u16())
}

/// PUT com body + content-type → propaga status.
pub async fn put_body(
    state: &AppState,
    base: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: Option<(&str, &str)>,
    body: Bytes,
    content_type: &str,
) -> WebResult<u16> {
    let url = build_url(base, path);
    let mut req = state
        .http
        .put(&url)
        .header("content-type", content_type)
        .body(body);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx {
        req = inject_ctx(req, t, u);
    }
    Ok(req.send().await?.status().as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_url_joins_base_and_path() {
        assert_eq!(
            build_url("https://api.example.com", "/v1/messages"),
            "https://api.example.com/v1/messages"
        );
    }

    #[test]
    fn build_url_trims_trailing_slash_from_base() {
        assert_eq!(
            build_url("https://api.example.com/", "/v1/users"),
            "https://api.example.com/v1/users"
        );
    }

    #[test]
    fn build_url_multiple_trailing_slashes_trimmed() {
        assert_eq!(
            build_url("https://svc.internal///", "/health"),
            "https://svc.internal/health"
        );
    }

    #[test]
    fn build_url_empty_path() {
        assert_eq!(
            build_url("https://svc.internal", ""),
            "https://svc.internal"
        );
    }

    #[test]
    fn build_url_base_without_trailing_slash() {
        assert_eq!(
            build_url("http://localhost:8080", "/api"),
            "http://localhost:8080/api"
        );
    }

    #[test]
    fn build_url_path_with_query() {
        assert_eq!(
            build_url("https://api.svc", "/items?limit=10"),
            "https://api.svc/items?limit=10"
        );
    }

    #[test]
    fn build_url_nested_path() {
        assert_eq!(
            build_url("https://api.svc/", "/v2/mail/messages/abc"),
            "https://api.svc/v2/mail/messages/abc"
        );
    }

    #[test]
    fn build_url_double_trailing_slash_on_base_trimmed() {
        assert_eq!(build_url("http://svc//", "/path"), "http://svc/path");
    }

    #[test]
    fn build_url_path_no_leading_slash() {
        assert_eq!(build_url("http://svc", "health"), "http://svchealth");
    }

    #[test]
    fn build_url_path_fragment_preserved() {
        assert_eq!(
            build_url("http://svc", "/api/v1/test"),
            "http://svc/api/v1/test"
        );
    }

    #[test]
    fn build_url_trailing_slash_on_base_is_stripped() {
        assert_eq!(build_url("http://svc/", "/path"), "http://svc/path");
    }

    #[test]
    fn build_url_deep_path_preserved() {
        assert_eq!(build_url("http://svc", "/a/b/c"), "http://svc/a/b/c");
    }

    #[test]
    fn build_url_root_path_produces_base_with_slash() {
        assert_eq!(build_url("http://svc", "/"), "http://svc/");
    }

    #[test]
    fn build_url_api_path_joins_correctly() {
        assert_eq!(build_url("http://svc", "/api/v1"), "http://svc/api/v1");
    }

    #[test]
    fn build_url_path_with_multiple_segments() {
        assert_eq!(
            build_url("http://svc", "/v1/mail/folders"),
            "http://svc/v1/mail/folders"
        );
    }

    #[test]
    fn build_url_empty_path_produces_base_only() {
        assert_eq!(build_url("http://svc", ""), "http://svc");
    }

    #[test]
    fn build_url_base_with_port_preserved() {
        assert_eq!(
            build_url("http://svc:9000", "/health"),
            "http://svc:9000/health"
        );
    }

    #[test]
    fn build_url_https_scheme_preserved() {
        assert_eq!(build_url("https://svc", "/ping"), "https://svc/ping");
    }

    #[test]
    fn build_url_path_without_leading_slash() {
        assert_eq!(build_url("http://svc", "health"), "http://svchealth");
    }

    #[test]
    fn build_url_path_with_slash_is_appended() {
        assert_eq!(build_url("http://svc", "/api/v1"), "http://svc/api/v1");
    }

    #[test]
    fn build_url_query_string_in_path_preserved() {
        assert_eq!(
            build_url("http://svc", "/search?q=foo&limit=5"),
            "http://svc/search?q=foo&limit=5"
        );
    }

    #[test]
    fn build_url_scheme_is_preserved_for_https() {
        let url = build_url("https://api.example.com", "/v1/status");
        assert!(url.starts_with("https://"));
    }

    #[test]
    fn build_url_path_and_base_concatenate_without_double_slash() {
        let url = build_url("https://api.example.com/", "/v2/resource");
        assert!(!url.contains("//v2"));
    }
}

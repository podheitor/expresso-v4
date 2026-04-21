//! Upstream HTTP client helpers — forward cookies + tenant/user context.

use crate::{AppState, error::{WebError, WebResult}};
use axum::http::HeaderMap;
use bytes::Bytes;
use reqwest::{StatusCode, RequestBuilder};
use serde::de::DeserializeOwned;

/// Copy `cookie` header from incoming request onto outgoing request.
pub fn fwd_cookie(mut req: RequestBuilder, headers: &HeaderMap) -> RequestBuilder {
    if let Some(c) = headers.get(axum::http::header::COOKIE).and_then(|v| v.to_str().ok()) {
        req = req.header("cookie", c);
    }
    req
}

/// Inject `x-tenant-id` + `x-user-id` headers from Me (when known).
pub fn inject_ctx(req: RequestBuilder, tenant_id: &str, user_id: &str) -> RequestBuilder {
    req.header("x-tenant-id", tenant_id)
       .header("x-user-id",   user_id)
}

fn build_url(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

/// GET JSON → Some(T) on 2xx, None on 401/403, Err on other failures.
pub async fn get_json<T: DeserializeOwned>(
    state:   &AppState,
    base:    &str,
    path:    &str,
    headers: &HeaderMap,
    ctx:     Option<(&str, &str)>,
) -> WebResult<Option<T>> {
    let url = build_url(base, path);
    let mut req = state.http.get(&url);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx { req = inject_ctx(req, t, u); }
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
    state:   &AppState,
    base:    &str,
    path:    &str,
    headers: &HeaderMap,
    ctx:     Option<(&str, &str)>,
) -> WebResult<u16> {
    let url = build_url(base, path);
    let mut req = state.http.post(&url).header("content-length", "0");
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx { req = inject_ctx(req, t, u); }
    Ok(req.send().await?.status().as_u16())
}

/// DELETE → propaga status code.
pub async fn delete_at(
    state:   &AppState,
    base:    &str,
    path:    &str,
    headers: &HeaderMap,
    ctx:     Option<(&str, &str)>,
) -> WebResult<u16> {
    let url = build_url(base, path);
    let mut req = state.http.delete(&url);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx { req = inject_ctx(req, t, u); }
    Ok(req.send().await?.status().as_u16())
}

/// POST com body + content-type → propaga status.
/// Utilizado p/ repassar multipart/form-data do browser p/ backend.
pub async fn post_body(
    state:        &AppState,
    base:         &str,
    path:         &str,
    headers:      &HeaderMap,
    ctx:          Option<(&str, &str)>,
    body:         Bytes,
    content_type: &str,
) -> WebResult<u16> {
    let url = build_url(base, path);
    let mut req = state.http.post(&url)
        .header("content-type", content_type)
        .body(body);
    req = fwd_cookie(req, headers);
    if let Some((t, u)) = ctx { req = inject_ctx(req, t, u); }
    Ok(req.send().await?.status().as_u16())
}

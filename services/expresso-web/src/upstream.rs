//! Upstream HTTP client helpers — forward cookies + propagate 401.

use crate::{AppState, error::WebResult};
use axum::http::HeaderMap;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;

/// Extract `cookie` header from incoming request → forward to upstream.
fn fwd_cookie(headers: &HeaderMap) -> Option<String> {
    headers.get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// GET JSON → Some(T) on 2xx, None on 401, Err on other failures.
pub async fn get_json<T: DeserializeOwned>(
    state:  &AppState,
    base:   &str,
    path:   &str,
    headers:&HeaderMap,
) -> WebResult<Option<T>> {
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let mut req = state.http.get(&url);
    if let Some(c) = fwd_cookie(headers) { req = req.header("cookie", c); }
    let resp = req.send().await?;
    match resp.status() {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Ok(None),
        s if s.is_success() => {
            let v = resp.json::<T>().await?;
            Ok(Some(v))
        }
        s => {
            let txt = resp.text().await.unwrap_or_default();
            Err(crate::error::WebError::Upstream(format!("{} {} → {}: {}", s, url, s, txt)))
        }
    }
}

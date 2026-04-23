//! RFC 6764 well-known URI for CalDAV service discovery.
//!
//! Clients (Thunderbird, iOS, macOS) probe `/.well-known/caldav` before the
//! user-supplied server URL. Per RFC 6764 we respond with `301 Moved
//! Permanently` to the principal collection root; clients then follow
//! `current-user-principal` via PROPFIND.

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/.well-known/caldav", any(redirect))
}

async fn redirect() -> Response {
    let mut resp = (StatusCode::MOVED_PERMANENTLY, "").into_response();
    resp.headers_mut()
        .insert(header::LOCATION, header::HeaderValue::from_static("/caldav/"));
    resp
}

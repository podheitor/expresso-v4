//! RFC 6764 well-known URI for CardDAV service discovery.

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/.well-known/carddav", any(redirect))
}

async fn redirect() -> Response {
    let mut resp = (StatusCode::MOVED_PERMANENTLY, "").into_response();
    resp.headers_mut()
        .insert(header::LOCATION, header::HeaderValue::from_static("/carddav/"));
    resp
}

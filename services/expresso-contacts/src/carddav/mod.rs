//! CardDAV (RFC 4791 subset) server implementation.
//!
//! Dispatches HTTP requests under `/carddav/*` to the appropriate handler based
//! on the request method (incl. non-standard PROPFIND / REPORT verbs).

mod auth;
mod propfind;
mod report;
mod resource;
mod uri;
mod xml;

use axum::{
    body::Body,
    extract::State,
    http::{Method, Request, StatusCode},
    response::Response,
    routing::any,
    Router,
};
use crate::carddav::auth::CardDavPrincipal;
use crate::state::AppState;

/// Multistatus content-type required by WebDAV (RFC 4918 §9.1).
pub const MULTISTATUS_CT: &str = "application/xml; charset=utf-8";

pub fn routes() -> Router<AppState> {
    // `any` matches every HTTP method; we branch on method inside the handler
    // because axum's MethodRouter does not ship with PROPFIND / REPORT verbs.
    Router::new()
        .route("/carddav",      any(dispatch))
        .route("/carddav/",     any(dispatch))
        .route("/carddav/*rest", any(dispatch))
}

async fn dispatch(
    State(state): State<AppState>,
    req: Request<Body>,
) -> Response {
    let method = req.method().clone();
    let path   = req.uri().path().to_owned();
    let headers = req.headers().clone();

    // Cheap method fast-paths first.
    if method == Method::OPTIONS {
        return resource::options();
    }

    // Auth: extract principal from headers BEFORE reading body, so we can fail early.
    let (mut parts, body) = req.into_parts();
    let principal = match CardDavPrincipal::from_request_parts_helper(&mut parts, &state).await {
        Ok(p)  => p,
        Err(r) => return r,
    };

    // Read body to string (8 MiB cap; iCal payloads are small, anything bigger → 413).
    const MAX_BODY: usize = 8 * 1024 * 1024;
    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b)  => b,
        Err(_) => return payload_too_large(),
    };
    let body_str = match std::str::from_utf8(&bytes) {
        Ok(s)  => s.to_owned(),
        Err(_) => return bad_request("body not utf-8"),
    };

    let result = match method_name(&method).as_str() {
 "PROPFIND" => {
            let depth = propfind::parse_depth(&headers);
            propfind::handle(state, principal, &path, depth, &body_str).await
        }
 "REPORT" => report::handle(state, principal, &path, &body_str).await,
 "GET"    => resource::get(state, principal, &path).await,
 "PUT"    => resource::put(state, principal, &path, body_str).await,
 "DELETE" => resource::delete(state, principal, &path).await,
 "HEAD"   => resource::get(state, principal, &path).await.map(|r| {
            // HEAD: strip body, keep headers.
            let (parts, _) = r.into_parts();
            Response::from_parts(parts, Body::empty())
        }),
        other => {
            return method_not_allowed(other);
        }
    };

    match result {
        Ok(resp) => resp,
        Err(e)   => axum::response::IntoResponse::into_response(e),
    }
}

/// Case-normalised method name; handles WebDAV-only verbs which are already
/// uppercase in `Method`, but this keeps any future client input safe.
fn method_name(m: &Method) -> String {
    m.as_str().to_ascii_uppercase()
}

fn bad_request(msg: &'static str) -> Response {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(Body::from(msg))
        .unwrap()
}

fn payload_too_large() -> Response {
    Response::builder()
        .status(StatusCode::PAYLOAD_TOO_LARGE)
        .body(Body::from("body exceeds 8 MiB"))
        .unwrap()
}

fn method_not_allowed(m: &str) -> Response {
    Response::builder()
        .status(StatusCode::METHOD_NOT_ALLOWED)
        .header("Allow", "OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, REPORT")
        .body(Body::from(format!("method not allowed: {m}")))
        .unwrap()
}

// ─── auth helper ────────────────────────────────────────────────────────────
// We can't invoke the extractor's `FromRequestParts::from_request_parts` with
// shared state directly from `dispatch` (borrow shape mismatch) — expose a
// thin helper that returns the error already converted to a Response.
impl CardDavPrincipal {
    async fn from_request_parts_helper(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Response> {
        use axum::extract::FromRequestParts;
        <Self as FromRequestParts<AppState>>::from_request_parts(parts, state)
            .await
            .map_err(axum::response::IntoResponse::into_response)
    }
}

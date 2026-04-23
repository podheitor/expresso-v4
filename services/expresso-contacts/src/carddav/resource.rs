//! CardDAV resource handlers — GET / PUT / DELETE on contact URIs.
//!
//! URI pattern: `/carddav/<user>/<addressbook>/<uid>.vcf`.
//! Body on PUT is raw vCard (text/vcard).

use axum::{
    body::Body,
    http::{header, HeaderValue, StatusCode},
    response::Response,
};

use crate::carddav::auth::CardDavPrincipal;
use crate::carddav::uri::{self, Target};
use crate::domain::ContactRepo;
use crate::error::Result;
use crate::state::AppState;

/// GET → return the stored vCard payload.
pub async fn get(
    state: AppState,
    principal: CardDavPrincipal,
    path: &str,
) -> Result<Response> {
    let (cal_id, uid) = match uri::classify(path) {
        Target::Contact { user_id, addressbook_id, uid } if user_id == principal.user_id =>
            (addressbook_id, uid),
        Target::Contact { .. } => return Ok(forbidden()),
        _ => return Ok(not_found()),
    };

    let pool = state.db_or_unavailable()?;
    let c = ContactRepo::new(pool).get_by_uid(principal.tenant_id, cal_id, &uid).await?;

    let resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/vcard; charset=utf-8")
        .header(header::ETAG, format!("\"{}\"", c.etag))
        .body(Body::from(c.vcard_raw))
        .unwrap();
    Ok(resp)
}

/// PUT → upsert contact from raw vCard body.
pub async fn put(
    state: AppState,
    principal: CardDavPrincipal,
    path: &str,
    body: String,
) -> Result<Response> {
    let cal_id = match uri::classify(path) {
        Target::Contact { user_id, addressbook_id, .. } if user_id == principal.user_id =>
            addressbook_id,
        Target::Contact { .. } => return Ok(forbidden()),
        _ => return Ok(not_found()),
    };

    let pool = state.db_or_unavailable()?;
    let c = ContactRepo::new(pool)
        .replace_by_uid(principal.tenant_id, cal_id, &body)
        .await?;

    let resp = Response::builder()
        .status(StatusCode::CREATED)
        .header(header::ETAG, format!("\"{}\"", c.etag))
        // No Content-Location (same as request URI by CardDAV convention).
        .body(Body::empty())
        .unwrap();
    Ok(resp)
}

/// DELETE → remove contact.
pub async fn delete(
    state: AppState,
    principal: CardDavPrincipal,
    path: &str,
) -> Result<Response> {
    let (cal_id, uid) = match uri::classify(path) {
        Target::Contact { user_id, addressbook_id, uid } if user_id == principal.user_id =>
            (addressbook_id, uid),
        Target::Contact { .. } => return Ok(forbidden()),
        _ => return Ok(not_found()),
    };

    let pool = state.db_or_unavailable()?;
    ContactRepo::new(pool).delete_by_uid(principal.tenant_id, cal_id, &uid).await?;

    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap())
}

/// OPTIONS → advertise supported DAV/CardDAV features.
pub fn options() -> Response {
    let mut resp = Response::builder()
        .status(StatusCode::OK)
        .header(
            header::ALLOW,
            "OPTIONS, GET, HEAD, PUT, DELETE, COPY, MOVE, PROPFIND, PROPPATCH, REPORT, MKCOL",
        )
        .body(Body::empty())
        .unwrap();
    resp.headers_mut().insert(
        "DAV",
        HeaderValue::from_static("1, 2, 3, addressbook"),
    );
    resp
}

fn forbidden() -> Response {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .body(Body::from("forbidden"))
        .unwrap()
}

fn not_found() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("not found"))
        .unwrap()
}

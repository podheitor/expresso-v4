//! CalDAV resource handlers — GET / PUT / DELETE on event URIs.
//!
//! URI pattern: `/caldav/<user>/<calendar>/<uid>.ics`.
//! Body on PUT is raw iCalendar (text/calendar).

use axum::{
    body::Body,
    http::{header, HeaderValue, StatusCode},
    response::Response,
};

use crate::caldav::auth::CalDavPrincipal;
use crate::caldav::uri::{self, Target};
use crate::domain::EventRepo;
use crate::error::Result;
use crate::state::AppState;

/// GET → return the stored iCalendar payload.
pub async fn get(
    state: AppState,
    principal: CalDavPrincipal,
    path: &str,
) -> Result<Response> {
    let (cal_id, uid) = match uri::classify(path) {
        Target::Event { user_id, calendar_id, uid } if user_id == principal.user_id =>
            (calendar_id, uid),
        Target::Event { .. } => return Ok(forbidden()),
        _ => return Ok(not_found()),
    };

    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get_by_uid(principal.tenant_id, cal_id, &uid).await?;

    let resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/calendar; charset=utf-8")
        .header(header::ETAG, format!("\"{}\"", ev.etag))
        .body(Body::from(ev.ical_raw))
        .unwrap();
    Ok(resp)
}

/// PUT → upsert event from raw iCalendar body.
pub async fn put(
    state: AppState,
    principal: CalDavPrincipal,
    path: &str,
    body: String,
) -> Result<Response> {
    let cal_id = match uri::classify(path) {
        Target::Event { user_id, calendar_id, .. } if user_id == principal.user_id =>
            calendar_id,
        Target::Event { .. } => return Ok(forbidden()),
        _ => return Ok(not_found()),
    };

    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool)
        .replace_by_uid(principal.tenant_id, cal_id, &body)
        .await?;

    let resp = Response::builder()
        .status(StatusCode::CREATED)
        .header(header::ETAG, format!("\"{}\"", ev.etag))
        // No Content-Location (same as request URI by CalDAV convention).
        .body(Body::empty())
        .unwrap();
    Ok(resp)
}

/// DELETE → remove event.
pub async fn delete(
    state: AppState,
    principal: CalDavPrincipal,
    path: &str,
) -> Result<Response> {
    let (cal_id, uid) = match uri::classify(path) {
        Target::Event { user_id, calendar_id, uid } if user_id == principal.user_id =>
            (calendar_id, uid),
        Target::Event { .. } => return Ok(forbidden()),
        _ => return Ok(not_found()),
    };

    let pool = state.db_or_unavailable()?;
    EventRepo::new(pool).delete_by_uid(principal.tenant_id, cal_id, &uid).await?;

    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap())
}

/// OPTIONS → advertise supported DAV/CalDAV features.
pub fn options() -> Response {
    let mut resp = Response::builder()
        .status(StatusCode::OK)
        .header(
            header::ALLOW,
            "OPTIONS, GET, HEAD, POST, PUT, DELETE, COPY, MOVE, PROPFIND, PROPPATCH, REPORT, MKCALENDAR",
        )
        .body(Body::empty())
        .unwrap();
    // DAV: header advertises supported feature classes.
    // calendar-schedule (RFC 6638) advertised; MVP delivers iTIP via SMTP.
    resp.headers_mut().insert(
        "DAV",
        HeaderValue::from_static("1, 2, 3, calendar-access, calendar-schedule"),
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

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
use crate::domain::{EventRepo, TaskRepo};
use crate::error::{CalendarError, Result};
use crate::events::Event;
use crate::state::AppState;

/// Resolve a CalDAV object URI to `(calendar_id, uid)` for the principal, or a
/// forbidden/not-found response when it isn't this user's event resource.
// Err is an axum Response (large by design — same as sibling CalDAV helpers).
#[allow(clippy::result_large_err)]
fn classify_object(
    path: &str,
    principal: &CalDavPrincipal,
) -> std::result::Result<(uuid::Uuid, String), Response> {
    match uri::classify(path) {
        Target::Event {
            user_id,
            calendar_id,
            uid,
        } if user_id == principal.user_id => Ok((calendar_id, uid)),
        Target::Event { .. } => Err(forbidden()),
        _ => Err(not_found()),
    }
}

/// GET → return the stored iCalendar payload. The `.ics` URI doesn't encode the
/// component type, so we look the UID up as an event first, then as a task.
pub async fn get(state: AppState, principal: CalDavPrincipal, path: &str) -> Result<Response> {
    let (cal_id, uid) = match classify_object(path, &principal) {
        Ok(v) => v,
        Err(resp) => return Ok(resp),
    };
    let pool = state.db_or_unavailable()?;

    if let Ok(ev) = EventRepo::new(pool)
        .get_by_uid(principal.tenant_id, cal_id, &uid)
        .await
    {
        return Ok(ical_response(&ev.etag, ev.ical_raw));
    }
    if let Ok(task) = TaskRepo::new(pool)
        .get_by_uid(principal.tenant_id, cal_id, &uid)
        .await
    {
        return Ok(ical_response(&task.caldav_etag(), task.to_ical()));
    }
    Ok(not_found())
}

/// PUT → upsert from a raw iCalendar body, routed by component type: a VTODO
/// body becomes a task, anything else an event (preserving prior behavior).
pub async fn put(
    state: AppState,
    principal: CalDavPrincipal,
    path: &str,
    body: String,
) -> Result<Response> {
    let cal_id = match classify_object(path, &principal) {
        Ok((cal_id, _)) => cal_id,
        Err(resp) => return Ok(resp),
    };
    let pool = state.db_or_unavailable()?;

    if is_vtodo(&body) {
        let task = TaskRepo::new(pool)
            .replace_by_uid(principal.tenant_id, cal_id, &body)
            .await?;
        return Ok(created_with_etag(&task.caldav_etag()));
    }

    let ev = EventRepo::new(pool)
        .replace_by_uid(principal.tenant_id, cal_id, &body)
        .await?;
    // Publish event (CalDAV PUT = upsert; emit as Updated with ev.sequence).
    state.events().publish(Event::EventUpdated {
        tenant_id: principal.tenant_id,
        event_id: ev.id,
        summary: ev.summary.clone(),
        sequence: ev.sequence,
    });
    state.events().publish_imip(ev.clone(), "REQUEST");
    Ok(created_with_etag(&ev.etag))
}

/// DELETE → remove the object. Tries the event store first (publishing a
/// cancellation), then the task store.
pub async fn delete(state: AppState, principal: CalDavPrincipal, path: &str) -> Result<Response> {
    let (cal_id, uid) = match classify_object(path, &principal) {
        Ok(v) => v,
        Err(resp) => return Ok(resp),
    };
    let pool = state.db_or_unavailable()?;

    let ev_id = EventRepo::new(pool)
        .get_by_uid(principal.tenant_id, cal_id, &uid)
        .await
        .ok()
        .map(|e| e.id);
    if let Some(event_id) = ev_id {
        EventRepo::new(pool)
            .delete_by_uid(principal.tenant_id, cal_id, &uid)
            .await?;
        state.events().publish(Event::EventCancelled {
            tenant_id: principal.tenant_id,
            event_id,
        });
        return Ok(no_content());
    }

    match TaskRepo::new(pool)
        .delete_by_uid(principal.tenant_id, cal_id, &uid)
        .await
    {
        Ok(()) => Ok(no_content()),
        Err(CalendarError::CalendarNotFound(_)) => Ok(not_found()),
        Err(e) => Err(e),
    }
}

/// True when the body's first calendar component is a VTODO.
fn is_vtodo(body: &str) -> bool {
    body.to_ascii_uppercase().contains("BEGIN:VTODO")
}

fn ical_response(etag: &str, ical: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/calendar; charset=utf-8")
        .header(header::ETAG, format!("\"{etag}\""))
        .body(Body::from(ical))
        .unwrap()
}

fn created_with_etag(etag: &str) -> Response {
    Response::builder()
        .status(StatusCode::CREATED)
        .header(header::ETAG, format!("\"{etag}\""))
        .body(Body::empty())
        .unwrap()
}

fn no_content() -> Response {
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap()
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

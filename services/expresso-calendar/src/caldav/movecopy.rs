//! CalDAV MOVE + COPY verbs (RFC 4918 §9.8 / §9.9).
//!
//! Scope: event resources only (URI pattern `/caldav/<user>/<calendar>/<uid>.ics`).
//! - Destination URI must resolve to the *same authenticated user*.
//! - Both source + destination must be Event targets.
//! - Cross-calendar COPY/MOVE allowed (same user, same tenant).
//! - `Overwrite: F` header → if destination exists, return 412.
//! - `Depth` ignored for resources (always 0).
//!
//! Out of scope (future): COPY/MOVE of whole collections, Depth: infinity.

use axum::{body::Body, http::{HeaderMap, StatusCode}, response::Response};

use crate::caldav::auth::CalDavPrincipal;
use crate::caldav::uri::{self, Target};
use crate::domain::EventRepo;
use crate::events::Event;
use crate::error::Result;
use crate::state::AppState;

/// COPY handler.
pub async fn copy(
    state:     AppState,
    principal: CalDavPrincipal,
    path:      &str,
    headers:   &HeaderMap,
) -> Result<Response> {
    process(state, principal, path, headers, /*is_move=*/ false).await
}

/// MOVE handler.
pub async fn mov(
    state:     AppState,
    principal: CalDavPrincipal,
    path:      &str,
    headers:   &HeaderMap,
) -> Result<Response> {
    process(state, principal, path, headers, /*is_move=*/ true).await
}

async fn process(
    state:     AppState,
    principal: CalDavPrincipal,
    path:      &str,
    headers:   &HeaderMap,
    is_move:   bool,
) -> Result<Response> {
    // Source must be an event owned by principal.
    let (src_cal, src_uid) = match uri::classify(path) {
        Target::Event { user_id, calendar_id, uid } if user_id == principal.user_id =>
            (calendar_id, uid),
        Target::Event { .. } => return Ok(simple(StatusCode::FORBIDDEN)),
        _ => return Ok(simple(StatusCode::NOT_FOUND)),
    };

    // Destination URI — parse from header, strip scheme+host if present.
    let dest_raw = match headers.get("destination").and_then(|h| h.to_str().ok()) {
        Some(s) => s.trim().to_string(),
        None => return Ok(bad_request("missing Destination header")),
    };
    let dest_path = strip_origin(&dest_raw);

    let (dst_cal, dst_uid) = match uri::classify(&dest_path) {
        Target::Event { user_id, calendar_id, uid } if user_id == principal.user_id =>
            (calendar_id, uid),
        Target::Event { .. } => return Ok(simple(StatusCode::FORBIDDEN)),
        _ => return Ok(bad_request("destination must resolve to an event URI")),
    };

    // Overwrite header — default T per RFC 4918 §10.6.
    let overwrite = headers.get("overwrite").and_then(|h| h.to_str().ok()).map(|s| s.trim());
    let allow_overwrite = !matches!(overwrite, Some("F") | Some("f"));

    let pool = state.db_or_unavailable()?;
    let repo = EventRepo::new(pool);

    // Fetch source.
    let src = repo.get_by_uid(principal.tenant_id, src_cal, &src_uid).await?;

    // Destination existence check.
    let dst_existed = repo
        .get_by_uid(principal.tenant_id, dst_cal, &dst_uid)
        .await
        .is_ok();
    if dst_existed && !allow_overwrite {
        return Ok(simple(StatusCode::PRECONDITION_FAILED));
    }

    // Write destination using source's raw iCalendar payload.
    // `replace_by_uid` parses the iCal — the UID inside the body may differ
    // from the URI `uid`; per RFC 4791 §4.1 the in-body UID is authoritative
    // and is what the row is keyed on. This matches PUT semantics.
    let dst_ev = repo
        .replace_by_uid(principal.tenant_id, dst_cal, &src.ical_raw)
        .await?;
    state.events().publish(Event::EventUpdated {
        tenant_id: principal.tenant_id,
        event_id:  dst_ev.id,
        summary:   dst_ev.summary.clone(),
        sequence:  dst_ev.sequence,
    });

    // If MOVE and source != destination row, delete source.
    let same_row = src_cal == dst_cal && src.uid == dst_uid;
    if is_move && !same_row {
        repo.delete_by_uid(principal.tenant_id, src_cal, &src.uid).await?;
        state.events().publish(Event::EventCancelled {
            tenant_id: principal.tenant_id,
            event_id:  src.id,
        });
    }

    let status = if dst_existed {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    };
    Ok(simple(status))
}

/// Strip absolute-URI scheme+authority (http://host[:port]) from Destination.
/// Returns a path starting with `/`.
fn strip_origin(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")) {
        // find first '/' after authority
        match rest.find('/') {
            Some(i) => rest[i..].to_string(),
            None => "/".to_string(),
        }
    } else {
        url.to_string()
    }
}

fn simple(status: StatusCode) -> Response {
    Response::builder().status(status).body(Body::empty()).unwrap()
}

fn bad_request(msg: &'static str) -> Response {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(Body::from(msg))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::strip_origin;

    #[test]
    fn strip_absolute_http() {
        assert_eq!(strip_origin("http://host:8002/caldav/u/c/x.ics"), "/caldav/u/c/x.ics");
        assert_eq!(strip_origin("https://h/caldav/u/c/x.ics"), "/caldav/u/c/x.ics");
    }

    #[test]
    fn keeps_path_only() {
        assert_eq!(strip_origin("/caldav/u/c/x.ics"), "/caldav/u/c/x.ics");
    }

    #[test]
    fn empty_authority_root() {
        assert_eq!(strip_origin("http://host"), "/");
    }
}

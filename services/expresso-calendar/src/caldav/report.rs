//! CalDAV REPORT handler — calendar-query + calendar-multiget.
//!
//! - calendar-query: filter events (currently supports `<time-range>`); returns
//!   matched events with requested props (typically getetag + calendar-data).
//! - calendar-multiget: fetch a list of explicit `<href>`s in one shot (client
//!   sends the hrefs returned by a previous PROPFIND/REPORT).

use axum::{body::Body, http::StatusCode, response::Response};
use time::{OffsetDateTime, PrimitiveDateTime, format_description};
use uuid::Uuid;

use crate::caldav::auth::CalDavPrincipal;
use crate::caldav::uri::{self, Target};
use crate::caldav::xml::{self, PropRequest};
use crate::caldav::MULTISTATUS_CT;
use crate::domain::{EventRepo, EventQuery};
use crate::error::Result;
use crate::state::AppState;

pub async fn handle(
    state:     AppState,
    principal: CalDavPrincipal,
    path:      &str,
    body:      &str,
) -> Result<Response> {
    // Only valid on calendar collection URIs.
    let calendar_id = match uri::classify(path) {
        Target::Calendar { user_id, calendar_id } if user_id == principal.user_id =>
            calendar_id,
        Target::Calendar { .. } => return Ok(forbidden()),
        _ => return Ok(not_found()),
    };

    // Detect REPORT variant by looking at the root element of the body.
    let lower = body.to_ascii_lowercase();
    let is_multiget = lower.contains("calendar-multiget");
    let is_query    = lower.contains("calendar-query");

    if !is_multiget && !is_query {
        return Ok(bad_request("unsupported REPORT variant"));
    }

    let req = xml::parse_propfind(body); // same prop-selection semantics
    let xml_out = if is_multiget {
        multiget(&state, &principal, calendar_id, body, &req).await?
    } else {
        query(&state, &principal, calendar_id, body, &req).await?
    };

    Ok(Response::builder()
        .status(StatusCode::from_u16(207).unwrap())
        .header("Content-Type", MULTISTATUS_CT)
        .body(Body::from(xml_out))
        .unwrap())
}

async fn multiget(
    state:       &AppState,
    principal:   &CalDavPrincipal,
    calendar_id: Uuid,
    body:        &str,
    req:         &PropRequest,
) -> Result<String> {
    let hrefs = xml::parse_multiget_hrefs(body);
    // Extract UIDs from hrefs that match our calendar path.
    let prefix = format!("/caldav/{}/{}/", principal.user_id, calendar_id);
    let mut uids: Vec<String> = hrefs
        .iter()
        .filter_map(|h| h.strip_prefix(&prefix))
        .filter_map(|s| s.strip_suffix(".ics"))
        .map(|s| uri::percent_decode(s))
        .collect();
    uids.sort();
    uids.dedup();

    let pool = state.db_or_unavailable()?;
    let events = EventRepo::new(pool)
        .list_by_uids(principal.tenant_id, calendar_id, &uids)
        .await?;

    let mut out = String::with_capacity(2048);
    out.push_str(xml::XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">"#);
    for ev in events {
        let href = format!("/caldav/{}/{}/{}.ics", principal.user_id, calendar_id, ev.uid);
        append_event(&mut out, &href, &ev.etag, req, &ev.ical_raw);
    }
    out.push_str("</D:multistatus>");
    Ok(out)
}

async fn query(
    state:       &AppState,
    principal:   &CalDavPrincipal,
    calendar_id: Uuid,
    body:        &str,
    req:         &PropRequest,
) -> Result<String> {
    let range = xml::parse_time_range(body).and_then(|(s, e)| {
        Some((parse_caldav_dt(&s)?, parse_caldav_dt(&e)?))
    });

    let q = EventQuery {
        from:  range.map(|(s, _)| s),
        to:    range.map(|(_, e)| e),
        limit: None,
    };

    let pool = state.db_or_unavailable()?;
    let events = EventRepo::new(pool)
        .list(principal.tenant_id, calendar_id, &q)
        .await?;

    let mut out = String::with_capacity(4096);
    out.push_str(xml::XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">"#);
    for ev in events {
        let href = format!("/caldav/{}/{}/{}.ics", principal.user_id, calendar_id, ev.uid);
        append_event(&mut out, &href, &ev.etag, req, &ev.ical_raw);
    }
    out.push_str("</D:multistatus>");
    Ok(out)
}

fn append_event(out: &mut String, href: &str, etag: &str, req: &PropRequest, ical: &str) {
    out.push_str("<D:response>");
    out.push_str("<D:href>"); out.push_str(&xml::escape(href)); out.push_str("</D:href>");
    out.push_str("<D:propstat><D:prop>");
    if req.getetag {
        out.push_str("<D:getetag>\"");
        out.push_str(&xml::escape(etag));
        out.push_str("\"</D:getetag>");
    }
    if req.getcontenttype {
        out.push_str(r#"<D:getcontenttype>text/calendar; charset=utf-8; component=VEVENT</D:getcontenttype>"#);
    }
    if req.calendar_data {
        out.push_str("<C:calendar-data>");
        out.push_str(&xml::escape(ical));
        out.push_str("</C:calendar-data>");
    }
    out.push_str("</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>");
    out.push_str("</D:response>");
}

/// Parse CalDAV time-range value "YYYYMMDDTHHMMSSZ" → OffsetDateTime (UTC).
fn parse_caldav_dt(v: &str) -> Option<OffsetDateTime> {
    let stripped = v.strip_suffix('Z').unwrap_or(v);
    let fmt = format_description::parse("[year][month][day]T[hour][minute][second]").ok()?;
    PrimitiveDateTime::parse(stripped, &fmt).ok().map(|p| p.assume_utc())
}

fn forbidden() -> Response {
    Response::builder().status(StatusCode::FORBIDDEN).body(Body::from("forbidden")).unwrap()
}
fn not_found() -> Response {
    Response::builder().status(StatusCode::NOT_FOUND).body(Body::from("not found")).unwrap()
}
fn bad_request(msg: &'static str) -> Response {
    Response::builder().status(StatusCode::BAD_REQUEST).body(Body::from(msg)).unwrap()
}

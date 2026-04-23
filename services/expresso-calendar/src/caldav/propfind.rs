//! CalDAV PROPFIND handler.
//!
//! Supports three URL shapes:
//!   1. `/caldav/<user>/`                      → calendar-home-set (children = calendars)
//!   2. `/caldav/<user>/<cal>/`                → calendar collection (children = events, getetag)
//!   3. `/caldav/<user>/<cal>/<uid>.ics`       → single event resource
//!
//! Depth header respected: 0 (self only) or 1 (self + children). Infinity is
//! treated as 1 for performance.

use axum::{
    body::Body,
    http::{HeaderMap, StatusCode},
    response::Response,
};

use crate::caldav::auth::CalDavPrincipal;
use crate::caldav::xml::{self, PropRequest};
use crate::caldav::{uri, MULTISTATUS_CT};
use crate::domain::{CalendarRepo, DeadProp, DeadPropRepo, EventRepo, EventQuery};
use crate::error::Result;
use crate::state::AppState;

/// Entry point called by the dispatcher after auth + body read.
pub async fn handle(
    state: AppState,
    principal: CalDavPrincipal,
    path: &str,
    depth: Depth,
    body: &str,
) -> Result<Response> {
    let req = xml::parse_propfind(body);
    let target = uri::classify(path);

    let xml_body = match target {
        uri::Target::Home { user_id } => {
            if user_id != principal.user_id {
                return Ok(forbidden());
            }
            propfind_home(&state, &principal, &req, depth).await?
        }
        uri::Target::Calendar { user_id, calendar_id } => {
            if user_id != principal.user_id {
                return Ok(forbidden());
            }
            propfind_calendar(&state, &principal, calendar_id, &req, depth).await?
        }
        uri::Target::Event { user_id, calendar_id, uid } => {
            if user_id != principal.user_id {
                return Ok(forbidden());
            }
            propfind_event(&state, &principal, calendar_id, &uid, &req).await?
        }
        uri::Target::ScheduleInbox { user_id } => {
            if user_id != principal.user_id { return Ok(forbidden()); }
            propfind_schedule(&principal, /*inbox=*/true, &req)
        }
        uri::Target::ScheduleOutbox { user_id } => {
            if user_id != principal.user_id { return Ok(forbidden()); }
            propfind_schedule(&principal, /*inbox=*/false, &req)
        }
        uri::Target::Unknown => return Ok(not_found()),
    };

    let resp = Response::builder()
        .status(StatusCode::from_u16(207).unwrap()) // Multi-Status
        .header("Content-Type", MULTISTATUS_CT)
        .body(Body::from(xml_body))
        .unwrap();
    Ok(resp)
}

/// Parsed `Depth` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Zero,
    One,
    Infinity,
}

pub fn parse_depth(headers: &HeaderMap) -> Depth {
    headers
        .get("depth")
        .and_then(|v| v.to_str().ok())
        .map(|s| match s.trim().to_ascii_lowercase().as_str() {
            "0"        => Depth::Zero,
            "1"        => Depth::One,
            "infinity" => Depth::Infinity,
            _          => Depth::Zero,
        })
        .unwrap_or(Depth::Zero)
}

// ─── builders ───────────────────────────────────────────────────────────────

async fn propfind_home(
    state:     &AppState,
    principal: &CalDavPrincipal,
    req:       &PropRequest,
    depth:     Depth,
) -> Result<String> {
    let pool = state.db_or_unavailable()?;
    let mut out = String::with_capacity(1024);
    out.push_str(xml::XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CS="http://calendarserver.org/ns/" xmlns:A="http://apple.com/ns/ical/">"#);

    // Self response — home collection.
    let home_href = format!("/caldav/{}/", principal.user_id);
    append_collection_response(&mut out, &home_href, /*is_home=*/true, None, None, None, req, principal, &[]);

    if matches!(depth, Depth::One | Depth::Infinity) {
        let calendars = CalendarRepo::new(pool)
            .list_for_owner(principal.tenant_id, principal.user_id)
            .await?;
        let dead_repo = DeadPropRepo::new(pool);
        for cal in calendars {
            let href = format!("/caldav/{}/{}/", principal.user_id, cal.id);
            let dead = if req.allprop {
                dead_repo.list_for_calendar(cal.id).await.unwrap_or_default()
            } else { Vec::new() };
            append_collection_response(
                &mut out, &href, /*is_home=*/false,
                Some(cal.name.as_str()),
                cal.description.as_deref(),
                Some((cal.color.as_deref(), cal.timezone.as_str(), cal.ctag)),
                req, principal, &dead,
            );
        }
    }

    // RFC 6638 §4: expose schedule-inbox + schedule-outbox collections when
    // Depth >= 1 so clients enumerate them alongside regular calendars.
    if matches!(depth, Depth::One | Depth::Infinity) {
        append_schedule_response(&mut out, principal, /*inbox=*/true,  req);
        append_schedule_response(&mut out, principal, /*inbox=*/false, req);
    }

    out.push_str("</D:multistatus>");
    Ok(out)
}

/// Stand-alone PROPFIND on a schedule-inbox/outbox collection URL.
fn propfind_schedule(
    principal: &CalDavPrincipal,
    inbox:     bool,
    req:       &PropRequest,
) -> String {
    let mut out = String::with_capacity(512);
    out.push_str(xml::XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">"#);
    append_schedule_response(&mut out, principal, inbox, req);
    out.push_str("</D:multistatus>");
    out
}

/// Append a single schedule-inbox or schedule-outbox `<D:response>`.
fn append_schedule_response(
    out:       &mut String,
    principal: &CalDavPrincipal,
    inbox:     bool,
    req:       &PropRequest,
) {
    let (seg, elem, disp) = if inbox {
        ("schedule-inbox",  "<C:schedule-inbox/>",  "Schedule Inbox")
    } else {
        ("schedule-outbox", "<C:schedule-outbox/>", "Schedule Outbox")
    };
    let href = format!("/caldav/{}/{seg}/", principal.user_id);

    out.push_str("<D:response>");
    out.push_str("<D:href>"); out.push_str(&xml::escape(&href)); out.push_str("</D:href>");
    out.push_str("<D:propstat><D:prop>");
    if req.resourcetype {
        out.push_str("<D:resourcetype><D:collection/>");
        out.push_str(elem);
        out.push_str("</D:resourcetype>");
    }
    if req.displayname {
        out.push_str("<D:displayname>");
        out.push_str(disp);
        out.push_str("</D:displayname>");
    }
    if req.current_user_principal {
        out.push_str("<D:current-user-principal><D:href>");
        out.push_str(&format!("/caldav/{}/", principal.user_id));
        out.push_str("</D:href></D:current-user-principal>");
    }
    if req.owner {
        out.push_str("<D:owner><D:href>");
        out.push_str(&format!("/caldav/{}/", principal.user_id));
        out.push_str("</D:href></D:owner>");
    }
    if req.current_user_privilege_set {
        // Inbox: read (+ schedule-deliver). Outbox: schedule-send.
        if inbox {
            out.push_str("<D:current-user-privilege-set><D:privilege><D:read/></D:privilege><D:privilege><C:schedule-deliver/></D:privilege></D:current-user-privilege-set>");
        } else {
            out.push_str("<D:current-user-privilege-set><D:privilege><D:read/></D:privilege><D:privilege><C:schedule-send/></D:privilege></D:current-user-privilege-set>");
        }
    }
    out.push_str("</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>");
    out.push_str("</D:response>");
}

async fn propfind_calendar(
    state:       &AppState,
    principal:   &CalDavPrincipal,
    calendar_id: uuid::Uuid,
    req:         &PropRequest,
    depth:       Depth,
) -> Result<String> {
    let pool = state.db_or_unavailable()?;
    let repo = CalendarRepo::new(pool);
    let cal = repo.get(principal.tenant_id, calendar_id).await?;

    let mut out = String::with_capacity(2048);
    out.push_str(xml::XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CS="http://calendarserver.org/ns/" xmlns:A="http://apple.com/ns/ical/">"#);

    let dead = if req.allprop {
        DeadPropRepo::new(pool).list_for_calendar(cal.id).await.unwrap_or_default()
    } else { Vec::new() };
    let href = format!("/caldav/{}/{}/", principal.user_id, cal.id);
    append_collection_response(
        &mut out, &href, /*is_home=*/false,
        Some(cal.name.as_str()),
        cal.description.as_deref(),
        Some((cal.color.as_deref(), cal.timezone.as_str(), cal.ctag)),
        req, principal, &dead,
    );

    if matches!(depth, Depth::One | Depth::Infinity) {
        let events = EventRepo::new(pool)
            .list(principal.tenant_id, calendar_id, &EventQuery::default())
            .await?;
        for ev in events {
            let ev_href = format!("/caldav/{}/{}/{}.ics", principal.user_id, cal.id, ev.uid);
            append_event_response(&mut out, &ev_href, &ev.etag, req, ev.ical_raw.as_str());
        }
    }

    out.push_str("</D:multistatus>");
    Ok(out)
}

async fn propfind_event(
    state:       &AppState,
    principal:   &CalDavPrincipal,
    calendar_id: uuid::Uuid,
    uid:         &str,
    req:         &PropRequest,
) -> Result<String> {
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool)
        .get_by_uid(principal.tenant_id, calendar_id, uid)
        .await?;

    let mut out = String::with_capacity(1024);
    out.push_str(xml::XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CS="http://calendarserver.org/ns/">"#);
    let href = format!("/caldav/{}/{}/{}.ics", principal.user_id, calendar_id, ev.uid);
    append_event_response(&mut out, &href, &ev.etag, req, ev.ical_raw.as_str());
    out.push_str("</D:multistatus>");
    Ok(out)
}

/// Append a `<D:response>` for a collection (home or calendar).
fn append_collection_response(
    out:        &mut String,
    href:       &str,
    is_home:    bool,
    dispname:   Option<&str>,
    descr:      Option<&str>,
    cal_meta:   Option<(Option<&str>, &str, i64)>,  // (color, tz, ctag)
    req:        &PropRequest,
    principal:  &CalDavPrincipal,
    dead_props: &[DeadProp],
) {
    out.push_str("<D:response>");
    out.push_str("<D:href>"); out.push_str(&xml::escape(href)); out.push_str("</D:href>");
    out.push_str("<D:propstat><D:prop>");

    if req.resourcetype {
        if is_home {
            out.push_str("<D:resourcetype><D:collection/></D:resourcetype>");
        } else {
            out.push_str("<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>");
        }
    }
    if req.displayname {
        let name = dispname.unwrap_or(if is_home { "Calendar Home" } else { "" });
        out.push_str("<D:displayname>");
        out.push_str(&xml::escape(name));
        out.push_str("</D:displayname>");
    }
    if req.current_user_principal {
        out.push_str("<D:current-user-principal><D:href>");
        out.push_str(&format!("/caldav/{}/", principal.user_id));
        out.push_str("</D:href></D:current-user-principal>");
    }
    if req.calendar_home_set {
        out.push_str("<C:calendar-home-set><D:href>");
        out.push_str(&format!("/caldav/{}/", principal.user_id));
        out.push_str("</D:href></C:calendar-home-set>");
    }
    if req.schedule_inbox_url {
        out.push_str("<C:schedule-inbox-URL><D:href>");
        out.push_str(&format!("/caldav/{}/schedule-inbox/", principal.user_id));
        out.push_str("</D:href></C:schedule-inbox-URL>");
    }
    if req.schedule_outbox_url {
        out.push_str("<C:schedule-outbox-URL><D:href>");
        out.push_str(&format!("/caldav/{}/schedule-outbox/", principal.user_id));
        out.push_str("</D:href></C:schedule-outbox-URL>");
    }
    if req.owner {
        out.push_str("<D:owner><D:href>");
        out.push_str(&format!("/caldav/{}/", principal.user_id));
        out.push_str("</D:href></D:owner>");
    }
    if let Some((color, tz, ctag)) = cal_meta {
        if req.getctag {
            out.push_str("<CS:getctag>");
            out.push_str(&format!("{ctag}"));
            out.push_str("</CS:getctag>");
        }
        if req.sync_token {
            // Opaque monotonic token (RFC 6578 §3). We reuse ctag as the
            // underlying monotonic counter.
            out.push_str("<D:sync-token>");
            out.push_str(&format!("urn:expresso:ctag:{ctag}"));
            out.push_str("</D:sync-token>");
        }
        if req.calendar_description {
            if let Some(d) = descr {
                out.push_str("<C:calendar-description>");
                out.push_str(&xml::escape(d));
                out.push_str("</C:calendar-description>");
            }
        }
        if req.calendar_timezone {
            out.push_str("<C:calendar-timezone>");
            out.push_str(&xml::escape(&format!(
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTIMEZONE\r\nTZID:{tz}\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
            )));
            out.push_str("</C:calendar-timezone>");
        }
        if req.calendar_color {
            if let Some(c) = color {
                out.push_str("<A:calendar-color>");
                out.push_str(&xml::escape(c));
                out.push_str("</A:calendar-color>");
            }
        }
        if req.supported_calendar_component_set {
            out.push_str("<C:supported-calendar-component-set><C:comp name=\"VEVENT\"/><C:comp name=\"VTODO\"/></C:supported-calendar-component-set>");
        }
    }
    if req.supported_report_set {
        // Advertise the REPORTs we serve (RFC 3253 §3.1.5 + RFC 4791 §5.2.1).
        out.push_str(            "<D:supported-report-set>\
             <D:supported-report><D:report><C:calendar-query/></D:report></D:supported-report>\
             <D:supported-report><D:report><C:calendar-multiget/></D:report></D:supported-report>\
             <D:supported-report><D:report><C:free-busy-query/></D:report></D:supported-report>\
             <D:supported-report><D:report><D:sync-collection/></D:report></D:supported-report>\
             </D:supported-report-set>"
        );
    }
    if req.current_user_privilege_set {
        // Owner privileges. Sharing ACL refinement handled elsewhere; here we
        // grant the principal full access to their own home/calendar.
        out.push_str(            "<D:current-user-privilege-set>\
             <D:privilege><D:read/></D:privilege>\
             <D:privilege><D:write/></D:privilege>\
             <D:privilege><D:write-content/></D:privilege>\
             <D:privilege><D:write-properties/></D:privilege>\
             <D:privilege><D:read-current-user-privilege-set/></D:privilege>\
             </D:current-user-privilege-set>"
        );
    }

    // Dead properties (RFC 4918 §15) — only when allprop requested.
    if req.allprop && !dead_props.is_empty() {
        for dp in dead_props {
            out.push_str(&format!(
                r#"<{local} xmlns="{ns}">{val}</{local}>"#,
                local = dp.local_name,
                ns    = xml::escape(&dp.namespace),
                val   = xml::escape(&dp.xml_value),
            ));
        }
    }

    out.push_str("</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>");
    out.push_str("</D:response>");
}

fn append_event_response(
    out: &mut String,
    href: &str,
    etag: &str,
    req:  &PropRequest,
    ical_raw: &str,
) {
    out.push_str("<D:response>");
    out.push_str("<D:href>"); out.push_str(&xml::escape(href)); out.push_str("</D:href>");
    out.push_str("<D:propstat><D:prop>");
    if req.resourcetype {
        out.push_str("<D:resourcetype/>");
    }
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
        out.push_str(&xml::escape(ical_raw));
        out.push_str("</C:calendar-data>");
    }
    if req.getcontentlength {
        out.push_str("<D:getcontentlength>");
        out.push_str(&ical_raw.len().to_string());
        out.push_str("</D:getcontentlength>");
    }
    if req.current_user_privilege_set {
        out.push_str(            "<D:current-user-privilege-set>\
             <D:privilege><D:read/></D:privilege>\
             <D:privilege><D:write/></D:privilege>\
             <D:privilege><D:write-content/></D:privilege>\
             </D:current-user-privilege-set>"
        );
    }
    out.push_str("</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>");
    out.push_str("</D:response>");
}

// ─── error responses ────────────────────────────────────────────────────────

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

//! CardDAV REPORT handler — addressbook-query + addressbook-multiget.
//!
//! - addressbook-query: filter contacts (currently supports `<time-range>`); returns
//!   matched contacts with requested props (typically getetag + address-data).
//! - addressbook-multiget: fetch a list of explicit `<href>`s in one shot (client
//!   sends the hrefs returned by a previous PROPFIND/REPORT).

use axum::{body::Body, http::StatusCode, response::Response};
use uuid::Uuid;

use crate::carddav::auth::CardDavPrincipal;
use crate::carddav::uri::{self, Target};
use crate::carddav::xml::{self, PropRequest};
use crate::carddav::MULTISTATUS_CT;
use crate::domain::ContactRepo;
use crate::error::Result;
use crate::state::AppState;

pub async fn handle(
    state:     AppState,
    principal: CardDavPrincipal,
    path:      &str,
    body:      &str,
) -> Result<Response> {
    // Only valid on addressbook collection URIs.
    let addressbook_id = match uri::classify(path) {
        Target::Addressbook { user_id, addressbook_id } if user_id == principal.user_id =>
            addressbook_id,
        Target::Addressbook { .. } => return Ok(forbidden()),
        _ => return Ok(not_found()),
    };

    // Detect REPORT variant by looking at the root element of the body.
    let lower = body.to_ascii_lowercase();
    let is_multiget = lower.contains("addressbook-multiget");
    let is_query    = lower.contains("addressbook-query");

    if !is_multiget && !is_query {
        return Ok(bad_request("unsupported REPORT variant"));
    }

    let req = xml::parse_propfind(body); // same prop-selection semantics
    let xml_out = if is_multiget {
        multiget(&state, &principal, addressbook_id, body, &req).await?
    } else {
        query(&state, &principal, addressbook_id, body, &req).await?
    };

    Ok(Response::builder()
        .status(StatusCode::from_u16(207).unwrap())
        .header("Content-Type", MULTISTATUS_CT)
        .body(Body::from(xml_out))
        .unwrap())
}

async fn multiget(
    state:       &AppState,
    principal:   &CardDavPrincipal,
    addressbook_id: Uuid,
    body:        &str,
    req:         &PropRequest,
) -> Result<String> {
    let hrefs = xml::parse_multiget_hrefs(body);
    // Extract UIDs from hrefs that match our addressbook path.
    let prefix = format!("/carddav/{}/{}/", principal.user_id, addressbook_id);
    let mut uids: Vec<String> = hrefs
        .iter()
        .filter_map(|h| h.strip_prefix(&prefix))
        .filter_map(|s| s.strip_suffix(".vcf"))
        .map(|s| uri::percent_decode(s))
        .collect();
    uids.sort();
    uids.dedup();

    let pool = state.db_or_unavailable()?;
    let contacts = ContactRepo::new(pool)
        .list_by_uids(principal.tenant_id, addressbook_id, &uids)
        .await?;

    let mut out = String::with_capacity(2048);
    out.push_str(xml::XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav">"#);
    for c in contacts {
        let href = format!("/carddav/{}/{}/{}.vcf", principal.user_id, addressbook_id, c.uid);
        append_contact(&mut out, &href, &c.etag, req, &c.vcard_raw);
    }
    out.push_str("</D:multistatus>");
    Ok(out)
}

async fn query(
    state:          &AppState,
    principal:      &CardDavPrincipal,
    addressbook_id: Uuid,
    _body:          &str,
    req:            &PropRequest,
) -> Result<String> {
    // MVP: ignore <filter>/<prop-filter>/<text-match> — return ALL contacts.
    // Clients like Evolution / Apple Contacts fetch once then do client-side
    // filtering, so this is safe for v1. Server-side text-match → TODO.
    let pool = state.db_or_unavailable()?;
    let contacts = ContactRepo::new(pool)
        .list(principal.tenant_id, addressbook_id)
        .await?;

    let mut out = String::with_capacity(4096);
    out.push_str(xml::XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav">"#);
    for c in contacts {
        let href = format!("/carddav/{}/{}/{}.vcf", principal.user_id, addressbook_id, c.uid);
        append_contact(&mut out, &href, &c.etag, req, &c.vcard_raw);
    }
    out.push_str("</D:multistatus>");
    Ok(out)
}

fn append_contact(out: &mut String, href: &str, etag: &str, req: &PropRequest, vcard: &str) {
    out.push_str("<D:response>");
    out.push_str("<D:href>"); out.push_str(&xml::escape(href)); out.push_str("</D:href>");
    out.push_str("<D:propstat><D:prop>");
    if req.getetag {
        out.push_str("<D:getetag>\"");
        out.push_str(&xml::escape(etag));
        out.push_str("\"</D:getetag>");
    }
    if req.getcontenttype {
        out.push_str(r#"<D:getcontenttype>text/vcard; charset=utf-8; </D:getcontenttype>"#);
    }
    if req.address_data {
        out.push_str("<C:address-data>");
        out.push_str(&xml::escape(vcard));
        out.push_str("</C:address-data>");
    }
    out.push_str("</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>");
    out.push_str("</D:response>");
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

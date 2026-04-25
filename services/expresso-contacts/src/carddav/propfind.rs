//! CardDAV PROPFIND handler.
//!
//! Supports three URL shapes:
//!   1. `/carddav/<user>/`                      → addressbook-home-set (children = addressbooks)
//!   2. `/carddav/<user>/<addressbook>/`                → addressbook collection (children = contacts, getetag)
//!   3. `/carddav/<user>/<addressbook>/<uid>.vcf`       → single contact resource
//!
//! Depth header respected: 0 (self only) or 1 (self + children). Infinity is
//! treated as 1 for performance.

use axum::{
    body::Body,
    http::{HeaderMap, StatusCode},
    response::Response,
};

use crate::carddav::auth::CardDavPrincipal;
use crate::carddav::xml::{self, PropRequest};
use crate::carddav::{uri, MULTISTATUS_CT};
use crate::domain::{AddressbookRepo, ContactRepo, DeadProp, DeadPropRepo};
use crate::error::Result;
use crate::state::AppState;

/// Entry point called by the dispatcher after auth + body read.
pub async fn handle(
    state: AppState,
    principal: CardDavPrincipal,
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
        uri::Target::Addressbook { user_id, addressbook_id } => {
            if user_id != principal.user_id {
                return Ok(forbidden());
            }
            propfind_addressbook(&state, &principal, addressbook_id, &req, depth).await?
        }
        uri::Target::Contact { user_id, addressbook_id, uid } => {
            if user_id != principal.user_id {
                return Ok(forbidden());
            }
            propfind_contact(&state, &principal, addressbook_id, &uid, &req).await?
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
    principal: &CardDavPrincipal,
    req:       &PropRequest,
    depth:     Depth,
) -> Result<String> {
    let pool = state.db_or_unavailable()?;
    let mut out = String::with_capacity(1024);
    out.push_str(xml::XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav"  >"#);

    // Self response — home collection.
    let home_href = format!("/carddav/{}/", principal.user_id);
    append_collection_response(&mut out, &home_href, /*is_home=*/true, None, None, None, req, principal, &[]);

    if matches!(depth, Depth::One | Depth::Infinity) {
        let books = AddressbookRepo::new(pool)
            .list_for_owner(principal.tenant_id, principal.user_id)
            .await?;
        let dead_repo = DeadPropRepo::new(pool);
        for ab in books {
            let href = format!("/carddav/{}/{}/", principal.user_id, ab.id);
            let dead = if req.allprop {
                dead_repo.list_for_addressbook(principal.tenant_id, ab.id).await.unwrap_or_default()
            } else { Vec::new() };
            append_collection_response(
                &mut out, &href, /*is_home=*/false,
                Some(ab.name.as_str()),
                ab.description.as_deref(),
                Some(ab.ctag),
                req, principal, &dead,
            );
        }
    }

    out.push_str("</D:multistatus>");
    Ok(out)
}

async fn propfind_addressbook(
    state:       &AppState,
    principal:   &CardDavPrincipal,
    addressbook_id: uuid::Uuid,
    req:         &PropRequest,
    depth:       Depth,
) -> Result<String> {
    let pool = state.db_or_unavailable()?;
    let repo = AddressbookRepo::new(pool);
    let ab = repo.get(principal.tenant_id, addressbook_id).await?;

    let mut out = String::with_capacity(2048);
    out.push_str(xml::XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav"  >"#);

    let dead = if req.allprop {
        DeadPropRepo::new(pool).list_for_addressbook(principal.tenant_id, ab.id).await.unwrap_or_default()
    } else { Vec::new() };
    let href = format!("/carddav/{}/{}/", principal.user_id, ab.id);
    append_collection_response(
        &mut out, &href, /*is_home=*/false,
        Some(ab.name.as_str()),
        ab.description.as_deref(),
        Some(ab.ctag),
        req, principal, &dead,
    );

    if matches!(depth, Depth::One | Depth::Infinity) {
        let contacts = ContactRepo::new(pool)
            .list(principal.tenant_id, addressbook_id)
            .await?;
        for c in contacts {
            let ev_href = format!("/carddav/{}/{}/{}.vcf", principal.user_id, ab.id, c.uid);
            append_contact_response(&mut out, &ev_href, &c.etag, req, c.vcard_raw.as_str());
        }
    }

    out.push_str("</D:multistatus>");
    Ok(out)
}

async fn propfind_contact(
    state:       &AppState,
    principal:   &CardDavPrincipal,
    addressbook_id: uuid::Uuid,
    uid:         &str,
    req:         &PropRequest,
) -> Result<String> {
    let pool = state.db_or_unavailable()?;
    let c = ContactRepo::new(pool)
        .get_by_uid(principal.tenant_id, addressbook_id, uid)
        .await?;

    let mut out = String::with_capacity(1024);
    out.push_str(xml::XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav" >"#);
    let href = format!("/carddav/{}/{}/{}.vcf", principal.user_id, addressbook_id, c.uid);
    append_contact_response(&mut out, &href, &c.etag, req, c.vcard_raw.as_str());
    out.push_str("</D:multistatus>");
    Ok(out)
}

/// Append a `<D:response>` for a collection (home or addressbook).
fn append_collection_response(
    out:        &mut String,
    href:       &str,
    is_home:    bool,
    dispname:   Option<&str>,
    descr:      Option<&str>,
    cal_meta:   Option<i64>,  // ctag when this is an addressbook collection
    req:        &PropRequest,
    principal:  &CardDavPrincipal,
    dead_props: &[DeadProp],
) {
    out.push_str("<D:response>");
    out.push_str("<D:href>"); out.push_str(&xml::escape(href)); out.push_str("</D:href>");
    out.push_str("<D:propstat><D:prop>");

    if req.resourcetype {
        if is_home {
            out.push_str("<D:resourcetype><D:collection/></D:resourcetype>");
        } else {
            out.push_str("<D:resourcetype><D:collection/><C:addressbook/></D:resourcetype>");
        }
    }
    if req.displayname {
        let name = dispname.unwrap_or(if is_home { "Addressbook Home" } else { "" });
        out.push_str("<D:displayname>");
        out.push_str(&xml::escape(name));
        out.push_str("</D:displayname>");
    }
    if req.current_user_principal {
        out.push_str("<D:current-user-principal><D:href>");
        out.push_str(&format!("/carddav/{}/", principal.user_id));
        out.push_str("</D:href></D:current-user-principal>");
    }
    if req.addressbook_home_set {
        out.push_str("<C:addressbook-home-set><D:href>");
        out.push_str(&format!("/carddav/{}/", principal.user_id));
        out.push_str("</D:href></C:addressbook-home-set>");
    }
    if req.owner {
        out.push_str("<D:owner><D:href>");
        out.push_str(&format!("/carddav/{}/", principal.user_id));
        out.push_str("</D:href></D:owner>");
    }
    if let Some(ctag) = cal_meta {
        if req.getctag {
            out.push_str("<CS:getctag>");
            out.push_str(&format!("{ctag}"));
            out.push_str("</CS:getctag>");
        }
        if req.addressbook_description {
            if let Some(d) = descr {
                out.push_str("<C:addressbook-description>");
                out.push_str(&xml::escape(d));
                out.push_str("</C:addressbook-description>");
            }
        }
        if req.supported_address_data {
            out.push_str("<C:supported-address-data><C:address-data-type content-type=\"text/vcard\" version=\"3.0\"/></C:supported-address-data>");
        }
        if req.sync_token {
            out.push_str("<D:sync-token>");
            out.push_str(&format!("urn:expresso:ctag:{ctag}"));
            out.push_str("</D:sync-token>");
        }
    }
    if req.supported_report_set {
        out.push_str(            "<D:supported-report-set>\
             <D:supported-report><D:report><C:addressbook-query/></D:report></D:supported-report>\
             <D:supported-report><D:report><C:addressbook-multiget/></D:report></D:supported-report>\
             <D:supported-report><D:report><D:sync-collection/></D:report></D:supported-report>\
             </D:supported-report-set>"
        );
    }
    if req.current_user_privilege_set {
        out.push_str(            "<D:current-user-privilege-set>\
             <D:privilege><D:read/></D:privilege>\
             <D:privilege><D:write/></D:privilege>\
             <D:privilege><D:write-content/></D:privilege>\
             <D:privilege><D:write-properties/></D:privilege>\
             <D:privilege><D:read-current-user-privilege-set/></D:privilege>\
             </D:current-user-privilege-set>"
        );
    }

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

fn append_contact_response(
    out: &mut String,
    href: &str,
    etag: &str,
    req:  &PropRequest,
    vcard_raw: &str,
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
        out.push_str(r#"<D:getcontenttype>text/vcard; charset=utf-8; </D:getcontenttype>"#);
    }
    if req.address_data {
        out.push_str("<C:address-data>");
        out.push_str(&xml::escape(vcard_raw));
        out.push_str("</C:address-data>");
    }
    if req.getcontentlength {
        out.push_str("<D:getcontentlength>");
        out.push_str(&vcard_raw.len().to_string());
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

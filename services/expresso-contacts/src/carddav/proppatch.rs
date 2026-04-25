//! CardDAV PROPPATCH handler.
//!
//! Live props on `addressbooks` columns:
//!   - `displayname`             → name
//!   - `addressbook-description` → description
//!
//! All other (namespace, local-name) pairs persist as **dead properties**
//! (RFC 4918 §15) in `addressbook_dead_properties`.

use axum::{body::Body, http::StatusCode, response::Response};
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

use crate::carddav::auth::CardDavPrincipal;
use crate::carddav::uri::{self, Target};
use crate::carddav::xml::{escape, XML_PROLOG};
use crate::carddav::MULTISTATUS_CT;
use crate::domain::{AddressbookRepo, DeadPropRepo, UpdateAddressbook};
use crate::error::Result;
use crate::state::AppState;

const LIVE_PROPS: &[(&str, &str)] = &[
    ("DAV:",                                  "displayname"),
    ("urn:ietf:params:xml:ns:carddav",        "addressbook-description"),
];

fn is_live_prop(ns: &str, local: &str) -> bool {
    let l = local.to_ascii_lowercase();
    LIVE_PROPS.iter().any(|(n, ln)| *n == ns && *ln == l)
}

pub async fn handle(
    state:     AppState,
    principal: CardDavPrincipal,
    path:      &str,
    body:      &str,
) -> Result<Response> {
    let addressbook_id = match uri::classify(path) {
        Target::Home { user_id } if user_id == principal.user_id => None,
        Target::Addressbook { user_id, addressbook_id } if user_id == principal.user_id => Some(addressbook_id),
        Target::Home { .. } | Target::Addressbook { .. } => return Ok(forbidden()),
        _ => return Ok(not_found()),
    };

    let (set_props, remove_props) = parse_set_remove(body);

    if let Some(id) = addressbook_id {
        // Live props → column mapping.
        let patch = build_patch(&set_props);
        if patch_has_changes(&patch) {
            let pool = state.db_or_unavailable()?;
            let _ = AddressbookRepo::new(pool)
                .update(principal.tenant_id, id, patch)
                .await;
        }

        // Dead props → persist.
        let has_dead = set_props.iter().any(|p| !is_live_prop(&p.namespace, &p.local))
                    || remove_props.iter().any(|p| !is_live_prop(&p.namespace, &p.local));
        if has_dead {
            let pool = state.db_or_unavailable()?;
            let repo = DeadPropRepo::new(pool);
            for p in &set_props {
                if !is_live_prop(&p.namespace, &p.local) {
                    let _ = repo.upsert_addressbook(
                        principal.tenant_id, id, &p.namespace, &p.local, &p.value,
                    ).await;
                }
            }
            for p in &remove_props {
                if !is_live_prop(&p.namespace, &p.local) {
                    let _ = repo.remove_addressbook(principal.tenant_id, id, &p.namespace, &p.local).await;
                }
            }
        }
    }

    let mut out = String::with_capacity(512);
    out.push_str(XML_PROLOG);
    out.push_str(
        r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav" xmlns:CS="http://calendarserver.org/ns/">"#,
    );
    out.push_str("<D:response>");
    out.push_str("<D:href>");
    out.push_str(&escape(path));
    out.push_str("</D:href>");

    for p in set_props.iter().chain(remove_props.iter()) {
        out.push_str("<D:propstat><D:prop>");
        out.push_str(&format!(
            r#"<{local} xmlns="{ns}"/>"#,
            local = p.local,
            ns    = escape(&p.namespace),
        ));
        out.push_str("</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>");
    }
    if set_props.is_empty() && remove_props.is_empty() {
        out.push_str("<D:propstat><D:prop/><D:status>HTTP/1.1 200 OK</D:status></D:propstat>");
    }
    out.push_str("</D:response></D:multistatus>");

    Ok(Response::builder()
        .status(StatusCode::from_u16(207).unwrap())
        .header("Content-Type", MULTISTATUS_CT)
        .body(Body::from(out))
        .unwrap())
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Prop {
    pub namespace: String,
    pub local:     String,
    pub value:     String,
}

fn parse_set_remove(body: &str) -> (Vec<Prop>, Vec<Prop>) {
    let mut reader = NsReader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut set_props    = Vec::new();
    let mut remove_props = Vec::new();
    let mut mode: Option<&'static str> = None;
    let mut in_prop = false;
    let mut current: Option<(String, String, String)> = None;

    loop {
        match reader.read_resolved_event() {
            Ok((nsr, Event::Start(e))) => {
                let local = local_name_str(&e);
                match local.to_ascii_lowercase().as_str() {
                    "set"    => mode = Some("set"),
                    "remove" => mode = Some("remove"),
                    "prop"   => in_prop = true,
                    _ if in_prop && current.is_none() => {
                        let ns = ns_to_string(&nsr);
                        current = Some((ns, local, String::new()));
                    }
                    _ => {}
                }
            }
            Ok((nsr, Event::Empty(e))) => {
                if in_prop {
                    let local = local_name_str(&e);
                    let ns    = ns_to_string(&nsr);
                    push_entry(&mut set_props, &mut remove_props, mode, ns, local, String::new());
                }
            }
            Ok((_, Event::Text(t))) => {
                if let Some((_, _, buf)) = current.as_mut() {
                    if let Ok(s) = t.decode() { buf.push_str(&s); }
                }
            }
            Ok((_, Event::End(e))) => {
                let name = e.name();
                let local = std::str::from_utf8(name.local_name().as_ref()).unwrap_or("").to_string();
                match local.to_ascii_lowercase().as_str() {
                    "set" | "remove" => mode = None,
                    "prop"           => in_prop = false,
                    _ => {
                        if let Some((ns, local, buf)) = current.take() {
                            push_entry(&mut set_props, &mut remove_props, mode, ns, local, buf.trim().to_string());
                        }
                    }
                }
            }
            Ok((_, Event::Eof)) => break,
            Err(_) => break,
            _ => {}
        }
    }
    (set_props, remove_props)
}

fn push_entry(
    set:    &mut Vec<Prop>,
    remove: &mut Vec<Prop>,
    mode:   Option<&'static str>,
    ns:     String,
    local:  String,
    value:  String,
) {
    let p = Prop { namespace: ns, local, value };
    match mode {
        Some("set")    => set.push(p),
        Some("remove") => remove.push(p),
        _ => {}
    }
}

fn local_name_str<'a>(e: &quick_xml::events::BytesStart<'a>) -> String {
    std::str::from_utf8(e.local_name().as_ref()).unwrap_or("").to_string()
}

fn ns_to_string(nsr: &ResolveResult<'_>) -> String {
    match nsr {
        ResolveResult::Bound(ns) => std::str::from_utf8(ns.as_ref()).unwrap_or("").to_string(),
        _ => String::new(),
    }
}

fn build_patch(set_props: &[Prop]) -> UpdateAddressbook {
    let mut patch = UpdateAddressbook::default();
    for p in set_props {
        if p.value.is_empty() { continue; }
        if !is_live_prop(&p.namespace, &p.local) { continue; }
        match p.local.to_ascii_lowercase().as_str() {
            "displayname"             => patch.name = Some(p.value.clone()),
            "addressbook-description" => patch.description = Some(p.value.clone()),
            _ => {}
        }
    }
    patch
}

fn patch_has_changes(p: &UpdateAddressbook) -> bool {
    p.name.is_some() || p.description.is_some()
}

fn forbidden() -> Response {
    Response::builder().status(StatusCode::FORBIDDEN).body(Body::from("forbidden")).unwrap()
}
fn not_found() -> Response {
    Response::builder().status(StatusCode::NOT_FOUND).body(Body::from("not found")).unwrap()
}

#[cfg(test)]
mod tests {
    use super::{build_patch, parse_set_remove, patch_has_changes, is_live_prop};

    #[test]
    fn parses_set_with_values() {
        let body = r#"<?xml version="1.0"?>
          <D:propertyupdate xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav">
            <D:set><D:prop>
              <D:displayname>Friends</D:displayname>
              <C:addressbook-description>close ones</C:addressbook-description>
            </D:prop></D:set>
          </D:propertyupdate>"#;
        let (set, _rem) = parse_set_remove(body);
        let dn = set.iter().find(|p| p.local == "displayname").unwrap();
        assert_eq!(dn.value, "Friends");
    }

    #[test]
    fn build_patch_maps_fields() {
        let body = r#"<D:propertyupdate xmlns:D="DAV:">
          <D:set><D:prop><D:displayname>Work</D:displayname></D:prop></D:set>
        </D:propertyupdate>"#;
        let (set, _) = parse_set_remove(body);
        let p = build_patch(&set);
        assert_eq!(p.name.as_deref(), Some("Work"));
        assert!(patch_has_changes(&p));
    }

    #[test]
    fn empty_patch_has_no_changes() {
        let p = super::UpdateAddressbook::default();
        assert!(!patch_has_changes(&p));
    }

    #[test]
    fn dead_prop_classification() {
        assert!(is_live_prop("DAV:", "displayname"));
        assert!(is_live_prop("urn:ietf:params:xml:ns:carddav", "addressbook-description"));
        assert!(!is_live_prop("http://example.com/x", "foo"));
    }

    #[test]
    fn parses_dead_props_with_custom_ns() {
        let body = r#"<D:propertyupdate xmlns:D="DAV:" xmlns:X="http://example.com/x">
          <D:set><D:prop><X:my-prop>hello</X:my-prop></D:prop></D:set>
        </D:propertyupdate>"#;
        let (set, _) = parse_set_remove(body);
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].namespace, "http://example.com/x");
        assert_eq!(set[0].local, "my-prop");
        assert_eq!(set[0].value, "hello");
    }
}

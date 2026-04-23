//! Minimal WebDAV/CardDAV XML helpers.
//!
//! WebDAV responses have a fixed, simple structure; we hand-build strings
//! with proper escaping. For *parsing* inbound PROPFIND/REPORT bodies we
//! walk quick-xml events (see `parse_*` fns).

use quick_xml::events::Event;
use quick_xml::reader::Reader;

/// XML text content escape (respects <, >, &, ", ').
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<'  => out.push_str("&lt;"),
            '>'  => out.push_str("&gt;"),
            '&'  => out.push_str("&amp;"),
            '"'  => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _    => out.push(c),
        }
    }
    out
}

/// Standard DAV multistatus response envelope (207).
pub const XML_PROLOG: &str = r#"<?xml version="1.0" encoding="utf-8"?>"#;

/// Property names we support in PROPFIND / REPORT responses.
#[derive(Debug, Clone, Default)]
pub struct PropRequest {
    pub displayname:                       bool,
    pub getetag:                           bool,
    pub getctag:                           bool,   // calendarserver ns/ (addressbook ctag)
    pub resourcetype:                      bool,
    pub getcontenttype:                    bool,
    pub current_user_principal:            bool,
    pub addressbook_home_set:                 bool,
    pub addressbook_description:              bool,
    pub supported_address_data:  bool,
    pub address_data:                     bool,   // carddav ns
    pub owner:                             bool,
    pub supported_report_set:              bool,
    pub current_user_privilege_set:        bool,
    pub getcontentlength:                  bool,
    pub sync_token:                        bool,
    /// True when body was empty or `<allprop/>` → include dead properties.
    pub allprop:                           bool,
}

impl PropRequest {
    /// Return every prop flag set (for `<allprop/>` or missing `<prop/>`).
    pub fn all() -> Self {
        Self {
            displayname: true,
            getetag: true,
            getctag: true,
            resourcetype: true,
            getcontenttype: true,
            current_user_principal: true,
            addressbook_home_set: true,
            addressbook_description: true,
            supported_address_data: true,
            address_data: true,
            owner: true,
            supported_report_set: true,
            current_user_privilege_set: true,
            getcontentlength: true,
            sync_token: true,
            allprop: true,
        }
    }
}

/// Parse PROPFIND body → which props are requested + allprop flag.
///
/// Accepts empty body (treated as allprop per RFC 4918 §9.1).
pub fn parse_propfind(body: &str) -> PropRequest {
    if body.trim().is_empty() {
        return PropRequest::all();
    }
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut req = PropRequest::default();
    let mut saw_allprop = false;
    let mut in_prop = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let local = local_name(e.name().as_ref());
                match local.as_str() {
 "allprop" => saw_allprop = true,
 "prop"    => in_prop = true,
                    name if in_prop => mark_prop(&mut req, name),
                    _ => {}
                }
                // Empty events don't open scope, so close prop manually:
                // (Actually prop is always Start+End; allprop is always Empty. No tracking needed.)
            }
            Ok(Event::End(e)) => {
                if local_name(e.name().as_ref()) == "prop" {
                    in_prop = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    if saw_allprop { PropRequest::all() } else { req }
}

/// Parse addressbook-multiget REPORT → list of `<href>` targets (paths).
pub fn parse_multiget_hrefs(body: &str) -> Vec<String> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut hrefs = Vec::new();
    let mut in_href = false;
    let mut buf = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == "href" {
                    in_href = true;
                    buf.clear();
                }
            }
            Ok(Event::Text(t)) if in_href => {
                if let Ok(s) = t.decode() {
                    buf.push_str(&s);
                }
            }
            Ok(Event::End(e)) => {
                if local_name(e.name().as_ref()) == "href" {
                    in_href = false;
                    if !buf.is_empty() {
                        hrefs.push(buf.trim().to_owned());
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    hrefs
}


/// Detect CardDAV REPORT variant by inspecting the root element.
pub fn detect_report_kind(body: &str) -> Option<&'static str> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                match local_name(e.name().as_ref()).as_str() {
                    "addressbook-query"    => return Some("addressbook-query"),
                    "addressbook-multiget" => return Some("addressbook-multiget"),
                    "sync-collection"      => return Some("sync-collection"),
                    _ => {}
                }
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// Extract `<sync-token>VALUE</sync-token>` from a sync-collection body.
/// Empty element or absent → None.
pub fn parse_sync_token(body: &str) -> Option<String> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut in_tok = false;
    let mut buf = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == "sync-token" {
                    in_tok = true;
                    buf.clear();
                }
            }
            Ok(Event::Text(t)) if in_tok => {
                if let Ok(txt) = t.decode() {
                    buf.push_str(&txt);
                }
            }
            Ok(Event::End(e)) => {
                if local_name(e.name().as_ref()) == "sync-token" {
                    let v = buf.trim().to_owned();
                    return if v.is_empty() { None } else { Some(v) };
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    None
}

// ─── helpers ────────────────────────────────────────────────────────────────

/// Strip namespace prefix from an XML element name (`C:prop` → `prop`).
fn local_name(bytes: &[u8]) -> String {
    let raw = std::str::from_utf8(bytes).unwrap_or("");
    raw.rsplit_once(':').map(|(_, l)| l).unwrap_or(raw).to_ascii_lowercase()
}

fn mark_prop(req: &mut PropRequest, name: &str) {
    match name {
 "displayname"                      => req.displayname = true,
 "getetag"                          => req.getetag = true,
 "getctag"                          => req.getctag = true,
 "resourcetype"                     => req.resourcetype = true,
 "getcontenttype"                   => req.getcontenttype = true,
 "current-user-principal"           => req.current_user_principal = true,
 "addressbook-home-set"                => req.addressbook_home_set = true,
 "addressbook-description"             => req.addressbook_description = true,
 "supported-address-data" => req.supported_address_data = true,
 "address-data"                    => req.address_data = true,
 "owner"                            => req.owner = true,
        "supported-report-set"             => req.supported_report_set = true,
        "current-user-privilege-set"       => req.current_user_privilege_set = true,
        "getcontentlength"                 => req.getcontentlength = true,
        "sync-token"                       => req.sync_token = true,
        _ => {}
    }
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_basic() {
        assert_eq!(escape("a<b&c>\"'"), "a&lt;b&amp;c&gt;&quot;&apos;");
    }

    #[test]
    fn propfind_allprop_empty() {
        let r = parse_propfind("");
        assert!(r.displayname && r.getetag && r.address_data);
    }

    #[test]
    fn propfind_named_props() {
        let xml = r#"<?xml version="1.0"?>
            <propfind xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav" >
              <prop><displayname/><getetag/><CS:getctag/><C:address-data/></prop>
            </propfind>"#;
        let r = parse_propfind(xml);
        assert!(r.displayname && r.getetag && r.getctag && r.address_data);
        assert!(!r.owner);
    }

    #[test]
    fn multiget_hrefs() {
        let xml = r#"<?xml version="1.0"?>
            <addressbook-multiget xmlns="urn:ietf:params:xml:ns:carddav" xmlns:D="DAV:">
              <D:prop><D:getetag/></D:prop>
              <D:href>/carddav/u/c/a.vcf</D:href>
              <D:href>/carddav/u/c/b.vcf</D:href>
            </addressbook-multiget>"#;
        let hrefs = parse_multiget_hrefs(xml);
        assert_eq!(hrefs, vec!["/carddav/u/c/a.vcf", "/carddav/u/c/b.vcf"]);
    }

    #[test]
    fn detect_report_kinds() {
        let q = r#"<addressbook-query xmlns="urn:ietf:params:xml:ns:carddav"/>"#;
        let m = r#"<addressbook-multiget xmlns="urn:ietf:params:xml:ns:carddav"/>"#;
        let sc = r#"<sync-collection xmlns="DAV:"/>"#;
        assert_eq!(super::detect_report_kind(q), Some("addressbook-query"));
        assert_eq!(super::detect_report_kind(m), Some("addressbook-multiget"));
        assert_eq!(super::detect_report_kind(sc), Some("sync-collection"));
        assert_eq!(super::detect_report_kind("<junk/>"), None);
    }

    #[test]
    fn parse_sync_token_present_and_empty() {
        let with = r#"<sync-collection xmlns="DAV:"><sync-token>urn:x:9</sync-token></sync-collection>"#;
        let empty = r#"<sync-collection xmlns="DAV:"><sync-token/></sync-collection>"#;
        assert_eq!(super::parse_sync_token(with), Some("urn:x:9".to_string()));
        assert_eq!(super::parse_sync_token(empty), None);
    }

    #[test]
    fn propfind_new_props() {
        let xml = r#"<propfind xmlns="DAV:"><prop><supported-report-set/><current-user-privilege-set/><getcontentlength/><sync-token/></prop></propfind>"#;
        let r = super::parse_propfind(xml);
        assert!(r.supported_report_set && r.current_user_privilege_set && r.getcontentlength && r.sync_token);
    }
}

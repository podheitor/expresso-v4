//! Minimal WebDAV/CalDAV XML helpers.
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
    pub getctag:                           bool,   // calendarserver ns
    pub resourcetype:                      bool,
    pub getcontenttype:                    bool,
    pub current_user_principal:            bool,
    pub calendar_home_set:                 bool,
    pub calendar_description:              bool,
    pub calendar_color:                    bool,   // apple ns
    pub calendar_timezone:                 bool,
    pub supported_calendar_component_set:  bool,
    pub calendar_data:                     bool,   // caldav ns
    pub owner:                             bool,
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
            calendar_home_set: true,
            calendar_description: true,
            calendar_color: true,
            calendar_timezone: true,
            supported_calendar_component_set: true,
            calendar_data: true,
            owner: true,
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

/// Parse calendar-multiget REPORT → list of `<href>` targets (paths).
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

/// Parse calendar-query time-range `<time-range start="..." end="..."/>`.
/// Returns (start, end) as raw strings (RFC 5545 DATE-TIME format YYYYMMDDTHHMMSSZ).
pub fn parse_time_range(body: &str) -> Option<(String, String)> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) == "time-range" {
                    let mut start = None;
                    let mut end   = None;
                    for attr in e.attributes().flatten() {
                        let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                        let val = attr.unescape_value().ok().map(|c| c.into_owned());
                        match key {
                            "start" => start = val,
                            "end"   => end   = val,
                            _ => {}
                        }
                    }
                    if let (Some(s), Some(e)) = (start, end) {
                        return Some((s, e));
                    }
                    return None;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
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
        "calendar-home-set"                => req.calendar_home_set = true,
        "calendar-description"             => req.calendar_description = true,
        "calendar-color"                   => req.calendar_color = true,
        "calendar-timezone"                => req.calendar_timezone = true,
        "supported-calendar-component-set" => req.supported_calendar_component_set = true,
        "calendar-data"                    => req.calendar_data = true,
        "owner"                            => req.owner = true,
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
        assert!(r.displayname && r.getetag && r.calendar_data);
    }

    #[test]
    fn propfind_named_props() {
        let xml = r#"<?xml version="1.0"?>
            <propfind xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CS="http://calendarserver.org/ns/">
              <prop><displayname/><getetag/><CS:getctag/><C:calendar-data/></prop>
            </propfind>"#;
        let r = parse_propfind(xml);
        assert!(r.displayname && r.getetag && r.getctag && r.calendar_data);
        assert!(!r.owner);
    }

    #[test]
    fn multiget_hrefs() {
        let xml = r#"<?xml version="1.0"?>
            <calendar-multiget xmlns="urn:ietf:params:xml:ns:caldav" xmlns:D="DAV:">
              <D:prop><D:getetag/></D:prop>
              <D:href>/caldav/u/c/a.ics</D:href>
              <D:href>/caldav/u/c/b.ics</D:href>
            </calendar-multiget>"#;
        let hrefs = parse_multiget_hrefs(xml);
        assert_eq!(hrefs, vec!["/caldav/u/c/a.ics", "/caldav/u/c/b.ics"]);
    }

    #[test]
    fn time_range_parse() {
        let xml = r#"<?xml version="1.0"?>
            <calendar-query xmlns="urn:ietf:params:xml:ns:caldav">
              <filter><comp-filter name="VCALENDAR"><comp-filter name="VEVENT">
                <time-range start="20260101T000000Z" end="20260131T235959Z"/>
              </comp-filter></comp-filter></filter>
            </calendar-query>"#;
        let (s, e) = parse_time_range(xml).unwrap();
        assert_eq!(s, "20260101T000000Z");
        assert_eq!(e, "20260131T235959Z");
    }
}

//! CardDAV `<addressbook-query>` filter parser + vCard matcher.
//!
//! RFC 6352 §10.5 — subset:
//!   * `<filter test="allof|anyof">` — default "anyof" per spec §10.5.1.
//!   * `<prop-filter name="FN" test="allof|anyof">` — default "anyof".
//!   * `<is-not-defined/>`
//!   * `<text-match match-type="contains|starts-with|ends-with|equals"
//!      negate-condition="yes|no" collation="...">text</text-match>`
//!
//! Collation handling: only `i;unicode-casemap` (the CardDAV default) and
//! `i;ascii-casemap` are honored — both implemented as ASCII case-insensitive
//! compare. Non-default collations fall back to the same behavior rather than
//! erroring, since real clients rarely send anything exotic.

use quick_xml::events::Event;
use quick_xml::reader::Reader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op { AllOf, AnyOf }

impl Default for Op { fn default() -> Self { Op::AnyOf } }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchType { Contains, StartsWith, EndsWith, Equals }

impl Default for MatchType { fn default() -> Self { Self::Contains } }

#[derive(Debug, Clone)]
pub struct TextMatch {
    pub value:      String,
    pub match_type: MatchType,
    pub negate:     bool,
}

#[derive(Debug, Clone, Default)]
pub struct PropFilter {
    pub name:           String,
    pub op:             Op,
    pub is_not_defined: bool,
    pub text_matches:   Vec<TextMatch>,
}

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub op:    Op,
    pub props: Vec<PropFilter>,
}

/// Parse `<filter>` from a REPORT addressbook-query body.
/// Returns None when the body has no `<filter>` element (→ no filtering).
pub fn parse(body: &str) -> Option<Filter> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);

    let mut found = false;
    let mut filter = Filter::default();
    // stack of current prop-filter being built
    let mut cur_pf: Option<PropFilter> = None;
    let mut cur_tm: Option<TextMatch>  = None;
    let mut tm_text = String::new();

    loop {
        match reader.read_event() {
            Ok(ev @ (Event::Start(_) | Event::Empty(_))) => {
                let is_empty = matches!(ev, Event::Empty(_));
                let e = match &ev {
                    Event::Start(e) | Event::Empty(e) => e.clone(),
                    _ => unreachable!(),
                };
                let name = local(e.name().as_ref());
                match name.as_str() {
                    "filter" => {
                        found = true;
                        filter.op = parse_test_attr(&e, Op::AnyOf);
                    }
                    "prop-filter" => {
                        let pf_name = attr(&e, "name").unwrap_or_default();
                        let pf = PropFilter {
                            name: pf_name,
                            op: parse_test_attr(&e, Op::AnyOf),
                            ..Default::default()
                        };
                        if is_empty {
                            filter.props.push(pf);
                        } else {
                            cur_pf = Some(pf);
                        }
                    }
                    "is-not-defined" => {
                        if let Some(pf) = cur_pf.as_mut() { pf.is_not_defined = true; }
                    }
                    "text-match" => {
                        let tm = TextMatch {
                            value: String::new(),
                            match_type: parse_match_type(&e),
                            negate: parse_negate(&e),
                        };
                        if is_empty {
                            if let Some(pf) = cur_pf.as_mut() { pf.text_matches.push(tm); }
                        } else {
                            cur_tm = Some(tm);
                            tm_text.clear();
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                if cur_tm.is_some() {
                    if let Ok(s) = t.decode() { tm_text.push_str(&s); }
                }
            }
            Ok(Event::End(e)) => {
                match local(e.name().as_ref()).as_str() {
                    "text-match" => {
                        if let (Some(mut tm), Some(pf)) = (cur_tm.take(), cur_pf.as_mut()) {
                            tm.value = tm_text.trim().to_owned();
                            pf.text_matches.push(tm);
                        }
                        tm_text.clear();
                    }
                    "prop-filter" => {
                        if let Some(pf) = cur_pf.take() {
                            filter.props.push(pf);
                        }
                    }
                    "filter" => break,
                    _ => {}
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    if found { Some(filter) } else { None }
}

// ─── attribute helpers ─────────────────────────────────────────────────────

fn local(bytes: &[u8]) -> String {
    let raw = std::str::from_utf8(bytes).unwrap_or("");
    raw.rsplit_once(':').map(|(_, l)| l).unwrap_or(raw).to_ascii_lowercase()
}

fn attr(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.as_ref().rsplit(|&b| b == b':').next() == Some(key.as_bytes()) {
            if let Ok(v) = a.unescape_value() {
                return Some(v.into_owned());
            }
        }
    }
    None
}

fn parse_test_attr(e: &quick_xml::events::BytesStart, default: Op) -> Op {
    match attr(e, "test").as_deref() {
        Some("anyof") => Op::AnyOf,
        Some("allof") => Op::AllOf,
        _             => default,
    }
}

fn parse_match_type(e: &quick_xml::events::BytesStart) -> MatchType {
    match attr(e, "match-type").as_deref() {
        Some("starts-with") => MatchType::StartsWith,
        Some("ends-with")   => MatchType::EndsWith,
        Some("equals")      => MatchType::Equals,
        _                   => MatchType::Contains,
    }
}

fn parse_negate(e: &quick_xml::events::BytesStart) -> bool {
    matches!(attr(e, "negate-condition").as_deref(), Some("yes"))
}

// ─── vCard matcher ─────────────────────────────────────────────────────────

/// Evaluate a `Filter` against a raw vCard 3.0/4.0 body.
pub fn matches(vcard: &str, f: &Filter) -> bool {
    let results: Vec<bool> = f.props.iter()
        .map(|pf| matches_prop(vcard, pf))
        .collect();
    combine(f.op, &results)
}

fn matches_prop(vcard: &str, pf: &PropFilter) -> bool {
    let vals = vcard_values(vcard, &pf.name);
    if pf.is_not_defined {
        return vals.is_empty();
    }
    if vals.is_empty() {
        return false;
    }
    if pf.text_matches.is_empty() {
        return true;
    }
    // A prop-filter matches if *any* occurrence of the property satisfies
    // the combined text-match conditions.
    vals.iter().any(|v| {
        let r: Vec<bool> = pf.text_matches.iter().map(|tm| tm_match(v, tm)).collect();
        combine(pf.op, &r)
    })
}

fn tm_match(value: &str, tm: &TextMatch) -> bool {
    let hay = value.to_ascii_lowercase();
    let needle = tm.value.to_ascii_lowercase();
    let hit = match tm.match_type {
        MatchType::Contains   => hay.contains(&needle),
        MatchType::StartsWith => hay.starts_with(&needle),
        MatchType::EndsWith   => hay.ends_with(&needle),
        MatchType::Equals     => hay == needle,
    };
    if tm.negate { !hit } else { hit }
}

fn combine(op: Op, results: &[bool]) -> bool {
    match op {
        Op::AllOf => results.iter().all(|&b| b),
        Op::AnyOf => results.iter().any(|&b| b),
    }
}

/// Return all values for the named vCard property (e.g. "FN" → ["Heitor Faria"]).
/// Ignores parameter section ("EMAIL;TYPE=WORK:x@y" → returns "x@y"). Line
/// folding (RFC 5545 §3.1) is not unfolded — real clients rarely fold short
/// props like FN/EMAIL and we only match on those.
fn vcard_values(vcard: &str, name: &str) -> Vec<String> {
    let upper = name.to_ascii_uppercase();
    vcard.lines()
        .filter_map(|line| {
            let (head, val) = line.split_once(':')?;
            let prop = head.split(';').next()?.trim().to_ascii_uppercase();
            if prop == upper { Some(val.to_owned()) } else { None }
        })
        .collect()
}

// ─── tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Heitor Faria\r\nEMAIL;TYPE=WORK:heitor@example.com\r\nEMAIL;TYPE=HOME:h@gmail.com\r\nEND:VCARD\r\n";

    #[test]
    fn parse_minimal_filter_fn_contains() {
        let body = r#"<?xml version="1.0"?>
            <C:addressbook-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav">
              <D:prop><D:getetag/></D:prop>
              <C:filter test="allof">
                <C:prop-filter name="FN">
                  <C:text-match match-type="contains">faria</C:text-match>
                </C:prop-filter>
              </C:filter>
            </C:addressbook-query>"#;
        let f = parse(body).expect("filter");
        assert_eq!(f.op, Op::AllOf);
        assert_eq!(f.props.len(), 1);
        assert_eq!(f.props[0].name, "FN");
        assert_eq!(f.props[0].text_matches.len(), 1);
        assert_eq!(f.props[0].text_matches[0].match_type, MatchType::Contains);
        assert_eq!(f.props[0].text_matches[0].value, "faria");
    }

    #[test]
    fn parse_no_filter_returns_none() {
        let body = r#"<C:addressbook-multiget xmlns:C="urn:ietf:params:xml:ns:carddav"/>"#;
        assert!(parse(body).is_none());
    }

    #[test]
    fn parse_is_not_defined() {
        let body = r#"<C:filter xmlns:C="urn:ietf:params:xml:ns:carddav">
            <C:prop-filter name="NICKNAME"><C:is-not-defined/></C:prop-filter>
        </C:filter>"#;
        let f = parse(body).expect("filter");
        assert!(f.props[0].is_not_defined);
    }

    #[test]
    fn parse_negate_and_match_types() {
        let body = r#"<C:filter xmlns:C="urn:ietf:params:xml:ns:carddav" test="anyof">
            <C:prop-filter name="EMAIL">
                <C:text-match match-type="starts-with" negate-condition="yes">bounce</C:text-match>
                <C:text-match match-type="ends-with">@example.com</C:text-match>
            </C:prop-filter>
        </C:filter>"#;
        let f = parse(body).expect("filter");
        assert_eq!(f.op, Op::AnyOf);
        assert_eq!(f.props[0].text_matches.len(), 2);
        assert!(f.props[0].text_matches[0].negate);
        assert_eq!(f.props[0].text_matches[0].match_type, MatchType::StartsWith);
        assert_eq!(f.props[0].text_matches[1].match_type, MatchType::EndsWith);
    }

    #[test]
    fn match_fn_contains_case_insensitive() {
        let f = Filter {
            op: Op::AllOf,
            props: vec![PropFilter {
                name: "FN".into(),
                op: Op::AnyOf,
                is_not_defined: false,
                text_matches: vec![TextMatch {
                    value: "HEITOR".into(),
                    match_type: MatchType::Contains,
                    negate: false,
                }],
            }],
        };
        assert!(matches(SAMPLE, &f));
    }

    #[test]
    fn match_email_ends_with() {
        let f = Filter {
            op: Op::AllOf,
            props: vec![PropFilter {
                name: "EMAIL".into(),
                op: Op::AnyOf,
                is_not_defined: false,
                text_matches: vec![TextMatch {
                    value: "@gmail.com".into(),
                    match_type: MatchType::EndsWith,
                    negate: false,
                }],
            }],
        };
        assert!(matches(SAMPLE, &f)); // home EMAIL matches
    }

    #[test]
    fn match_is_not_defined_true_when_absent() {
        let f = Filter {
            op: Op::AllOf,
            props: vec![PropFilter {
                name: "NICKNAME".into(),
                is_not_defined: true,
                ..Default::default()
            }],
        };
        assert!(matches(SAMPLE, &f));
    }

    #[test]
    fn match_is_not_defined_false_when_present() {
        let f = Filter {
            op: Op::AllOf,
            props: vec![PropFilter {
                name: "FN".into(),
                is_not_defined: true,
                ..Default::default()
            }],
        };
        assert!(!matches(SAMPLE, &f));
    }

    #[test]
    fn match_negate_condition_inverts() {
        let f = Filter {
            op: Op::AllOf,
            props: vec![PropFilter {
                name: "FN".into(),
                op: Op::AnyOf,
                is_not_defined: false,
                text_matches: vec![TextMatch {
                    value: "xyz".into(),
                    match_type: MatchType::Contains,
                    negate: true,
                }],
            }],
        };
        assert!(matches(SAMPLE, &f));
    }

    #[test]
    fn match_root_anyof_short_circuits() {
        let f = Filter {
            op: Op::AnyOf,
            props: vec![
                PropFilter {
                    name: "NICKNAME".into(),
                    is_not_defined: false,
                    op: Op::AllOf,
                    text_matches: vec![TextMatch {
                        value: "x".into(),
                        match_type: MatchType::Equals,
                        negate: false,
                    }],
                },
                PropFilter {
                    name: "FN".into(),
                    op: Op::AnyOf,
                    is_not_defined: false,
                    text_matches: vec![TextMatch {
                        value: "heitor".into(),
                        match_type: MatchType::Contains,
                        negate: false,
                    }],
                },
            ],
        };
        assert!(matches(SAMPLE, &f));
    }
}

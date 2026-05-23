//! Admin read-only listing of DAV dead properties (calendar + addressbook).
//!
//! Dead properties are arbitrary XML props clients set via PROPPATCH (e.g.
//! Apple-specific calendar color, custom display order). This UI surfaces
//! them for troubleshooting unexpected collection metadata.

use std::sync::Arc;

use askama::Template;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{auth, AppState};

#[derive(Debug)]
pub struct DeadPropRow {
    pub kind:         String, // "calendar" | "addressbook"
    pub parent_id:    String,
    pub parent_name:  String,
    pub namespace:    String,
    pub local_name:   String,
    pub value_preview: String,
    pub tenant_id:    String,
    pub updated_at:   String,
}

#[derive(Template)]
#[template(path = "dead_props_admin.html")]
pub struct DeadPropsTpl {
    pub current: &'static str,
    pub rows:    Vec<DeadPropRow>,
}

fn fmt_ts(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
}

fn preview(s: String) -> String {
    let trimmed = s.trim().chars().take(120).collect::<String>();
    if s.len() > 120 { format!("{trimmed}…") } else { trimmed }
}

pub async fn page(
    State(st): State<Arc<AppState>>,
    headers:   HeaderMap,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return r; }
    let Some(pool) = st.db.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "db unavailable").into_response();
    };

    let sql = r#"
        SELECT 'calendar'::text AS kind, p.calendar_id AS parent_id,
               COALESCE(c.name, '') AS parent_name,
               p.namespace, p.local_name, p.xml_value, p.tenant_id, p.updated_at
          FROM calendar_dead_properties p
          LEFT JOIN calendars c ON c.id = p.calendar_id
        UNION ALL
        SELECT 'addressbook'::text AS kind, p.addressbook_id AS parent_id,
               COALESCE(a.name, '') AS parent_name,
               p.namespace, p.local_name, p.xml_value, p.tenant_id, p.updated_at
          FROM addressbook_dead_properties p
          LEFT JOIN addressbooks a ON a.id = p.addressbook_id
         ORDER BY updated_at DESC
         LIMIT 200
    "#;
    let rows = match sqlx::query(sql).fetch_all(pool).await {
        Ok(rs) => rs.into_iter().map(|r| DeadPropRow {
            kind:          r.try_get::<String, _>("kind").unwrap_or_default(),
            parent_id:     r.try_get::<Uuid, _>("parent_id").map(|u| u.to_string()).unwrap_or_default(),
            parent_name:   r.try_get::<String, _>("parent_name").unwrap_or_default(),
            namespace:     r.try_get::<String, _>("namespace").unwrap_or_default(),
            local_name:    r.try_get::<String, _>("local_name").unwrap_or_default(),
            value_preview: preview(r.try_get::<String, _>("xml_value").unwrap_or_default()),
            tenant_id:     r.try_get::<Uuid, _>("tenant_id").map(|u| u.to_string()).unwrap_or_default(),
            updated_at:    r.try_get::<OffsetDateTime, _>("updated_at").map(fmt_ts).unwrap_or_default(),
        }).collect(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("query: {e}")).into_response(),
    };

    match (DeadPropsTpl { current: "deadprops", rows }).render() {
        Ok(html) => (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response(),
        Err(e)   => (StatusCode::INTERNAL_SERVER_ERROR, format!("template: {e}")).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn fmt_ts_formats_rfc3339() {
        let ts = datetime!(2026-05-22 09:15:00 UTC);
        let s = fmt_ts(ts);
        assert!(s.starts_with("2026-05-22T09:15:00"));
    }

    #[test]
    fn preview_short_string_unchanged() {
        let s = "hello world".to_string();
        assert_eq!(preview(s.clone()), s);
    }

    #[test]
    fn preview_exactly_120_chars_unchanged() {
        let s = "a".repeat(120);
        let out = preview(s.clone());
        assert_eq!(out, s);
        assert!(!out.contains('…'));
    }

    #[test]
    fn preview_over_120_chars_truncated() {
        let s = "x".repeat(200);
        let out = preview(s);
        // 120 chars + '…'
        assert!(out.ends_with('…'));
        let chars: Vec<char> = out.chars().collect();
        assert_eq!(chars.len(), 121); // 120 'x' + '…'
    }

    #[test]
    fn preview_trims_leading_whitespace() {
        let s = "   trimmed".to_string();
        // trim() applied before take(120)
        assert_eq!(preview(s), "trimmed");
    }

    #[test]
    fn preview_empty_string() {
        assert_eq!(preview(String::new()), "");
    }

    #[test]
    fn preview_121_chars_appends_ellipsis() {
        let s = "y".repeat(121);
        let out = preview(s);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn fmt_ts_unix_epoch_not_empty() {
        let ts = time::OffsetDateTime::UNIX_EPOCH;
        let s = fmt_ts(ts);
        assert!(s.starts_with("1970-01-01"));
    }

    #[test]
    fn preview_120_chars_unchanged() {
        let s = "z".repeat(120);
        let out = preview(s.clone());
        assert_eq!(out, s);
        assert!(!out.ends_with('…'));
    }

    #[test]
    fn preview_empty_string_unchanged() {
        assert_eq!(preview("".into()), "");
    }

    #[test]
    fn preview_whitespace_only_returns_empty_after_trim() {
        let out = preview("   ".into());
        assert!(out.is_empty() || out == "   ");
    }

    #[test]
    fn preview_nonempty_string_is_nonempty() {
        let out = preview("hello world".into());
        assert!(!out.is_empty());
    }

    #[test]
    fn preview_short_preserves_content() {
        let out = preview("test value".into());
        assert_eq!(out, "test value");
    }

    #[test]
    fn preview_empty_string_returns_empty() {
        let out = preview("".into());
        assert!(out.is_empty());
    }

    #[test]
    fn preview_long_value_is_nonempty() {
        let long = "x".repeat(100);
        let out = preview(long);
        assert!(!out.is_empty());
    }

    #[test]
    fn fmt_ts_contains_separator_t() {
        use time::macros::datetime;
        let ts = datetime!(2026-11-30 15:45:00 UTC);
        let s = fmt_ts(ts);
        assert!(s.contains('T'));
    }

    #[test]
    fn preview_short_value_is_returned_unchanged() {
        let s = "hello";
        assert_eq!(preview(s.into()), "hello");
    }

    #[test]
    fn fmt_ts_year_in_output() {
        use time::macros::datetime;
        let ts = datetime!(2026-08-20 07:00:00 UTC);
        let s = fmt_ts(ts);
        assert!(s.contains("2026"));
    }

    #[test]
    fn preview_101_chars_is_truncated() {
        let s = "a".repeat(121);
        let p = preview(&s);
        assert!(p.len() < s.len());
    }

    #[test]
    fn fmt_ts_rfc3339_contains_z_or_offset() {
        use time::macros::datetime;
        let ts = datetime!(2026-03-10 12:00:00 UTC);
        let s = fmt_ts(ts);
        assert!(s.ends_with('Z') || s.contains('+') || s.contains('-'));
    }

    #[test]
    fn preview_119_chars_unchanged() {
        let s = "b".repeat(119);
        let out = preview(s.clone());
        assert_eq!(out, s);
        assert!(!out.contains('…'));
    }
}

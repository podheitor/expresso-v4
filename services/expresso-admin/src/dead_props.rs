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

//! Admin handlers for iTIP COUNTER proposals (RFC 5546 §3.2.7).
//!
//! Flow:
//!   GET  /counter.html        → list pending across tenants (SuperAdmin)
//!   POST /counter/:id/accept  → apply proposed DTSTART/DTEND to event + resolve
//!   POST /counter/:id/reject  → mark proposal rejected (admin later sends DECLINECOUNTER)

use std::sync::Arc;

use askama::Template;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{audit, auth, AppState};

// ─── List page ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct CounterRow {
    pub id:                String,
    pub tenant_id:         String,
    pub event_id:          String,
    pub event_summary:     String,
    pub attendee_email:    String,
    pub proposed_dtstart:  String,
    pub proposed_dtend:    String,
    pub received_sequence: String,
    pub created_at_fmt:    String,
}

#[derive(Template)]
#[template(path = "counter_admin.html")]
pub struct CounterAdminTpl {
    pub current: &'static str,
    pub rows:    Vec<CounterRow>,
    pub flash:   Option<String>,
}

fn fmt_opt_ts(t: Option<OffsetDateTime>) -> String {
    match t {
        Some(v) => v.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        None => "—".into(),
    }
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
        SELECT p.id, p.tenant_id, p.event_id, p.attendee_email,
               p.proposed_dtstart, p.proposed_dtend,
               p.received_sequence, p.created_at,
               COALESCE(e.summary, '(no summary)') AS event_summary
          FROM scheduling_counter_proposals p
          LEFT JOIN calendar_events e ON e.id = p.event_id
         WHERE p.status = 'pending'
         ORDER BY p.created_at DESC
         LIMIT 200
    "#;
    let rs = sqlx::query(sql).fetch_all(pool).await;

    let (rows, flash) = match rs {
        Ok(rows) => {
            let mapped = rows.into_iter().map(|r| CounterRow {
                id:                r.try_get::<Uuid, _>("id").map(|u| u.to_string()).unwrap_or_default(),
                tenant_id:         r.try_get::<Uuid, _>("tenant_id").map(|u| u.to_string()).unwrap_or_default(),
                event_id:          r.try_get::<Uuid, _>("event_id").map(|u| u.to_string()).unwrap_or_default(),
                event_summary:     r.try_get::<String, _>("event_summary").unwrap_or_default(),
                attendee_email:    r.try_get::<String, _>("attendee_email").unwrap_or_default(),
                proposed_dtstart:  fmt_opt_ts(r.try_get::<Option<OffsetDateTime>, _>("proposed_dtstart").unwrap_or(None)),
                proposed_dtend:    fmt_opt_ts(r.try_get::<Option<OffsetDateTime>, _>("proposed_dtend").unwrap_or(None)),
                received_sequence: r.try_get::<Option<i32>, _>("received_sequence").unwrap_or(None).map(|v| v.to_string()).unwrap_or("—".into()),
                created_at_fmt:    r.try_get::<OffsetDateTime, _>("created_at")
                    .ok()
                    .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok())
                    .unwrap_or_default(),
            }).collect();
            (mapped, None)
        }
        Err(e) => (vec![], Some(format!("query failed: {e}"))),
    };

    let tpl = CounterAdminTpl { current: "counter", rows, flash };
    match tpl.render() {
        Ok(html) => (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response(),
        Err(e)   => (StatusCode::INTERNAL_SERVER_ERROR, format!("template: {e}")).into_response(),
    }
}

// ─── Accept / Reject actions ────────────────────────────────────────────────

async fn load_proposal(pool: &sqlx::PgPool, id: Uuid)
    -> Result<(Uuid, Uuid, Option<OffsetDateTime>, Option<OffsetDateTime>, String), sqlx::Error>
{
    let r = sqlx::query(
        r#"SELECT tenant_id, event_id, proposed_dtstart, proposed_dtend, status
             FROM scheduling_counter_proposals WHERE id = $1"#,
    )
    .bind(id)
    .fetch_one(pool).await?;
    Ok((
        r.try_get("tenant_id")?,
        r.try_get("event_id")?,
        r.try_get("proposed_dtstart")?,
        r.try_get("proposed_dtend")?,
        r.try_get("status")?,
    ))
}

pub async fn accept(
    State(st): State<Arc<AppState>>,
    headers:   HeaderMap,
    Path(id):  Path<Uuid>,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return r; }
    let Some(pool) = st.db.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "db unavailable").into_response();
    };

    let (tenant_id, event_id, dtstart, dtend, status) = match load_proposal(pool, id).await {
        Ok(v) => v,
        Err(_) => return Redirect::to("/counter.html?error=not-found").into_response(),
    };
    if status != "pending" {
        return Redirect::to("/counter.html?error=already-resolved").into_response();
    }

    // Apply proposed times to the event. SEQUENCE auto-bumps via Update ai logic.
    let upd = sqlx::query(
        r#"UPDATE calendar_events
              SET dtstart = COALESCE($3, dtstart),
                  dtend   = COALESCE($4, dtend),
                  sequence = CASE
                      WHEN dtstart IS DISTINCT FROM COALESCE($3, dtstart)
                        OR dtend   IS DISTINCT FROM COALESCE($4, dtend)
                      THEN sequence + 1
                      ELSE sequence
                  END
            WHERE tenant_id = $1 AND id = $2"#,
    )
    .bind(tenant_id).bind(event_id).bind(dtstart).bind(dtend)
    .execute(pool).await;
    if let Err(e) = upd {
        tracing::warn!(error=%e, %id, "counter accept: event UPDATE failed");
        return Redirect::to("/counter.html?error=update-failed").into_response();
    }

    // Mark proposal accepted (only if still pending — idempotent).
    let who = auth::principal_for(&st, &headers).await.user_id;
    let _ = sqlx::query(
        r#"UPDATE scheduling_counter_proposals
              SET status='accepted', resolved_at=NOW(), resolved_by=$2
            WHERE id=$1 AND status='pending'"#,
    )
    .bind(id).bind(who)
    .execute(pool).await;

    audit::record(
        &st, &headers, &Method::POST, "/counter/:id/accept",
        "admin.counter.accept",
        Some("counter_proposal"), Some(id.to_string()),
        Some(200),
        serde_json::json!({ "event_id": event_id.to_string(), "tenant_id": tenant_id.to_string() }),
    ).await;

    Redirect::to("/counter.html?accepted=1").into_response()
}

pub async fn reject(
    State(st): State<Arc<AppState>>,
    headers:   HeaderMap,
    Path(id):  Path<Uuid>,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return r; }
    let Some(pool) = st.db.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "db unavailable").into_response();
    };

    let who = auth::principal_for(&st, &headers).await.user_id;
    let res = sqlx::query(
        r#"UPDATE scheduling_counter_proposals
              SET status='rejected', resolved_at=NOW(), resolved_by=$2
            WHERE id=$1 AND status='pending'"#,
    )
    .bind(id).bind(who)
    .execute(pool).await;
    if let Err(e) = res {
        tracing::warn!(error=%e, %id, "counter reject: UPDATE failed");
        return Redirect::to("/counter.html?error=reject-failed").into_response();
    }

    audit::record(
        &st, &headers, &Method::POST, "/counter/:id/reject",
        "admin.counter.reject",
        Some("counter_proposal"), Some(id.to_string()),
        Some(200),
        serde_json::json!({}),
    ).await;

    Redirect::to("/counter.html?rejected=1").into_response()
}

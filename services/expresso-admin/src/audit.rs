//! Audit helper for admin service — wires principal + handler context into
//! `expresso_core::audit::record_async`, keeping handlers one-liner at the
//! end of mutation flows.

use std::sync::Arc;
use axum::http::{HeaderMap, Method};
use serde_json::Value as JsonValue;

use crate::{auth, AppState};

/// Build + fire-and-forget insert an audit row for an admin mutation.
/// No-op when DB pool is missing (admin can run in KC-only mode).
pub async fn record(
    st:         &Arc<AppState>,
    headers:    &HeaderMap,
    method:     &Method,
    http_path:  &str,
    action:     &str,
    target_type: Option<&str>,
    target_id:   Option<String>,
    status_code: Option<i16>,
    metadata:    JsonValue,
) {
    let Some(pool) = st.db.clone() else { return };
    let principal = auth::principal_for(st, headers).await;
    let entry = expresso_core::audit::AuditEntry {
        tenant_id:   principal.tenant_id,
        actor_sub:   principal.user_id.map(|u| u.to_string()),
        actor_email: principal.email,
        actor_roles: principal.roles,
        action:      action.to_string(),
        target_type: target_type.map(str::to_string),
        target_id,
        http_method: Some(method.as_str().to_string()),
        http_path:   Some(http_path.to_string()),
        status_code,
        metadata,
    };
    expresso_core::audit::record_async(pool, entry);
}


// ─── GET /audit listing endpoint ────────────────────────────────────────────

use axum::{extract::{Query, State}, response::{IntoResponse, Response}, Json};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    /// Filter by exact action LIKE prefix (e.g. `admin.tenant`).
    #[serde(default)]
    pub action_prefix: Option<String>,
    /// Filter by tenant UUID.
    #[serde(default)]
    pub tenant_id: Option<uuid::Uuid>,
    /// Max rows (1..=500, default 50).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Preset window: `24h` | `7d` | `30d` | `all`. Overrides `since`/`until` when set.
    #[serde(default)]
    pub preset: Option<String>,
    /// Inclusive lower bound (RFC3339).
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub since: Option<time::OffsetDateTime>,
    /// Exclusive upper bound (RFC3339).
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub until: Option<time::OffsetDateTime>,
    /// Cursor: fetch rows with `id < before_id` (next page).
    #[serde(default)]
    pub before_id: Option<i64>,
    /// Flash: rows purged (after POST /audit/purge redirect).
    #[serde(default)]
    pub purged: Option<i64>,
    /// Flash: retention days used in purge.
    #[serde(default)]
    pub days:   Option<i32>,
    /// Flash: error code (e.g. `db-unavailable`, `purge-failed`).
    #[serde(default)]
    pub error:  Option<String>,
}

/// Resolve `preset` → concrete `(since, until)` pair; `since/until` query params
/// take effect only when preset is None/empty/`custom`.
fn resolve_window(q: &AuditQuery) -> (Option<time::OffsetDateTime>, Option<time::OffsetDateTime>) {
    let now = time::OffsetDateTime::now_utc();
    match q.preset.as_deref() {
        Some("24h") => (Some(now - time::Duration::hours(24)), None),
        Some("7d")  => (Some(now - time::Duration::days(7)),   None),
        Some("30d") => (Some(now - time::Duration::days(30)),  None),
        Some("all") => (None, None),
        _ => (q.since, q.until),
    }
}


#[derive(Debug, Serialize)]
pub struct AuditRow {
    pub id:         i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub tenant_id:  uuid::Uuid,
    pub user_id:    Option<uuid::Uuid>,
    pub action:     String,
    pub resource:   Option<String>,
    pub status:     String,
    pub metadata:   serde_json::Value,
}

pub async fn list(
    State(st): State<Arc<AppState>>,
    headers:   HeaderMap,
    Query(q):  Query<AuditQuery>,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return r; }
    let Some(pool) = st.db.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "database unavailable").into_response();
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let prefix_like = q.action_prefix.as_ref().map(|p| format!("{p}%"));

    let (since, until) = resolve_window(&q);
    let rows = sqlx::query(
        r#"SELECT id, created_at, tenant_id, user_id, action, resource, status, metadata
             FROM audit_log
            WHERE ($1::uuid        IS NULL OR tenant_id  = $1)
              AND ($2::text        IS NULL OR action     LIKE $2)
              AND ($3::timestamptz IS NULL OR created_at >= $3)
              AND ($4::timestamptz IS NULL OR created_at <  $4)
              AND ($5::bigint      IS NULL OR id         <  $5)
            ORDER BY id DESC
            LIMIT $6"#,
    )
    .bind(q.tenant_id)
    .bind(prefix_like.as_deref())
    .bind(since)
    .bind(until)
    .bind(q.before_id)
    .bind(limit)
    .fetch_all(pool).await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")).into_response(),
    };

    let out: Vec<AuditRow> = rows.into_iter().map(|r| AuditRow {
        id:         r.try_get("id").unwrap_or_default(),
        created_at: r.try_get("created_at").unwrap_or_else(|_| OffsetDateTime::UNIX_EPOCH),
        tenant_id:  r.try_get("tenant_id").unwrap_or_else(|_| uuid::Uuid::nil()),
        user_id:    r.try_get("user_id").ok().flatten(),
        action:     r.try_get("action").unwrap_or_default(),
        resource:   r.try_get("resource").ok().flatten(),
        status:     r.try_get("status").unwrap_or_else(|_| String::from("unknown")),
        metadata:   r.try_get("metadata").unwrap_or(serde_json::json!({})),
    }).collect();

    Json(out).into_response()
}


// ─── GET /audit.html — SuperAdmin HTML page ────────────────────────────────

use askama::Template;
use crate::templates::{AuditAdminTpl, AuditViewRow};

pub async fn page(
    State(st): State<Arc<AppState>>,
    headers:   HeaderMap,
    Query(q):  Query<AuditQuery>,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return r; }
    let limit = q.limit.unwrap_or(50).clamp(1, 500);

    // Reuse the same query shape as `list` but return rendered template.
    let (rows, err) = match st.db.as_ref() {
        None => (vec![], Some("Database unavailable".to_string())),
        Some(pool) => {
            let prefix_like = q.action_prefix.as_ref().map(|p| format!("{p}%"));
            let (since, until) = resolve_window(&q);
            let res = sqlx::query(
                r#"SELECT id, created_at, tenant_id, user_id, action, resource, status, metadata
                     FROM audit_log
                    WHERE ($1::uuid        IS NULL OR tenant_id  = $1)
                      AND ($2::text        IS NULL OR action     LIKE $2)
                      AND ($3::timestamptz IS NULL OR created_at >= $3)
                      AND ($4::timestamptz IS NULL OR created_at <  $4)
                      AND ($5::bigint      IS NULL OR id         <  $5)
                    ORDER BY id DESC
                    LIMIT $6"#,
            )
            .bind(q.tenant_id)
            .bind(prefix_like.as_deref())
            .bind(since)
            .bind(until)
            .bind(q.before_id)
            .bind(limit)
            .fetch_all(pool).await;
            match res {
                Ok(rr) => {
                    let fmt = time::format_description::well_known::Rfc3339;
                    let rows: Vec<AuditViewRow> = rr.into_iter().map(|r| {
                        let ts: time::OffsetDateTime = r.try_get("created_at").unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
                        let meta: serde_json::Value = r.try_get("metadata").unwrap_or(serde_json::json!({}));
                        AuditViewRow {
                            id:             r.try_get("id").unwrap_or_default(),
                            created_at_fmt: ts.format(&fmt).unwrap_or_default(),
                            tenant_id:      r.try_get::<uuid::Uuid, _>("tenant_id").map(|u| u.to_string()).unwrap_or_default(),
                            user_id:        r.try_get::<Option<uuid::Uuid>, _>("user_id").ok().flatten().map(|u| u.to_string()),
                            action:         r.try_get("action").unwrap_or_default(),
                            resource:       r.try_get("resource").ok().flatten(),
                            status:         r.try_get("status").unwrap_or_else(|_| "unknown".into()),
                            metadata_json:  serde_json::to_string(&meta).unwrap_or_default(),
                        }
                    }).collect();
                    (rows, None)
                }
                Err(e) => (vec![], Some(format!("db: {e}"))),
            }
        }
    };

    // Build query string for the JSON shortcut link, preserving filters.
    let preset_v = q.preset.clone().unwrap_or_default();
    let fmt_dt = |dt: Option<time::OffsetDateTime>| dt
        .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_default();
    let since_v = fmt_dt(q.since);
    let until_v = fmt_dt(q.until);

    let mut qs_parts: Vec<String> = Vec::new();
    if let Some(p) = &q.action_prefix { if !p.is_empty() { qs_parts.push(format!("action_prefix={}", p.replace(' ', "%20").replace('&', "%26"))); } }
    if let Some(t) = q.tenant_id { qs_parts.push(format!("tenant_id={t}")); }
    if !preset_v.is_empty()       { qs_parts.push(format!("preset={preset_v}")); }
    if !since_v.is_empty()        { qs_parts.push(format!("since={}",  since_v.replace(':', "%3A").replace('+', "%2B"))); }
    if !until_v.is_empty()        { qs_parts.push(format!("until={}",  until_v.replace(':', "%3A").replace('+', "%2B"))); }
    qs_parts.push(format!("limit={limit}"));
    let query_string = format!("?{}", qs_parts.join("&"));

    // Compute cursor for "older" page: id of the last (smallest) row shown.
    let next_before_id: Option<i64> = rows.last().map(|r| r.id);
    let next_href = if let Some(bid) = next_before_id {
        // Replace/append before_id in query_string.
        let mut parts: Vec<String> = query_string
            .trim_start_matches('?')
            .split('&')
            .filter(|p| !p.is_empty() && !p.starts_with("before_id="))
            .map(|s| s.to_string())
            .collect();
        parts.push(format!("before_id={bid}"));
        Some(format!("/audit.html?{}", parts.join("&")))
    } else { None };

    // `reset_href`: same filters but without cursor (jump back to newest).
    let reset_parts: Vec<String> = query_string
        .trim_start_matches('?')
        .split('&')
        .filter(|p| !p.is_empty() && !p.starts_with("before_id="))
        .map(|s| s.to_string())
        .collect();
    let reset_href = format!("/audit.html?{}", reset_parts.join("&"));

    let has_cursor = q.before_id.is_some();

    // Build flash message from query params (purge redirect / errors).
    let flash: Option<String> = match (q.purged, q.days, q.error.as_deref(), &err) {
        (Some(n), Some(d), _, _) => Some(format!("Purge concluído: {n} row(s) removida(s) (retenção {d}d).")),
        (_, _, Some("db-unavailable"), _) => Some("Purge falhou: pool de DB indisponível.".to_string()),
        (_, _, Some("purge-failed"),   _) => Some("Purge falhou: erro ao executar audit_log_purge().".to_string()),
        (_, _, _, Some(e))               => Some(e.clone()),
        _ => None,
    };

    let tpl = AuditAdminTpl {
        current:         "audit",
        rows,
        limit,
        action_prefix_v: q.action_prefix.clone().unwrap_or_default(),
        tenant_id_v:     q.tenant_id.map(|t| t.to_string()).unwrap_or_default(),
        preset_v,
        since_v,
        until_v,
        query_string,
        next_href,
        reset_href,
        has_cursor,
        error:           flash,
    };
    match tpl.render() {
        Ok(html) => (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response(),
        Err(e)   => (StatusCode::INTERNAL_SERVER_ERROR, format!("template: {e}")).into_response(),
    }
}

// --- Retention purge -----------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PurgeForm {
    pub retention_days: i32,
}

/// POST /audit/purge — SuperAdmin-only. Invokes `audit_log_purge(N)` and
/// logs the purge itself as an audit event `admin.audit.purge`.
pub async fn purge(
    State(st):   State<Arc<AppState>>,
    headers:     HeaderMap,
    axum::Form(f): axum::Form<PurgeForm>,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return r; }

    // Clamp sane bounds server-side (defensive: UI restricts 7..3650).
    let days = f.retention_days.clamp(1, 3650);

    let pool = match st.db.as_ref() {
        Some(p) => p,
        None => {
            return axum::response::Redirect::to("/audit.html?error=db-unavailable")
                .into_response();
        }
    };

    let row = sqlx::query("SELECT audit_log_purge($1) AS deleted")
        .bind(days)
        .fetch_one(pool).await;

    let deleted: i64 = match row {
        Ok(r)  => r.try_get("deleted").unwrap_or(0),
        Err(e) => {
            tracing::warn!(error=%e, "audit_log_purge call failed");
            return axum::response::Redirect::to("/audit.html?error=purge-failed")
                .into_response();
        }
    };

    // Audit the purge itself (survives since we insert AFTER the delete on the same table).
    record(
        &st, &headers, &Method::POST, "/audit/purge",
        "admin.audit.purge",
        Some("audit_log"), None,
        Some(200),
        serde_json::json!({ "retention_days": days, "deleted": deleted }),
    ).await;

    axum::response::Redirect::to(
        &format!("/audit.html?purged={deleted}&days={days}")
    ).into_response()
}


// --- CSV export ---------------------------------------------------------

/// Escape a field for CSV output.
///
/// Combines RFC 4180 quoting (comma/quote/CR/LF) with a guard against
/// CSV/Excel formula injection: campos que começam com `=`, `+`, `-`, `@`,
/// `\t` ou `\r` recebem um prefixo `'` para que Excel/Google Sheets/LibreOffice
/// tratem o conteúdo como string literal em vez de fórmula. Sem essa proteção,
/// um atacante que controle qualquer campo do audit (action, resource,
/// metadata) consegue executar fórmulas — incluindo HYPERLINK pra exfiltrar
/// dados — quando um super_admin abre o CSV exportado.
fn csv_escape(f: &str) -> String {
    let starts_dangerous = f.as_bytes().first()
        .is_some_and(|b| matches!(*b, b'=' | b'+' | b'-' | b'@' | b'\t' | b'\r'));
    let needs_quote = starts_dangerous
        || f.contains(',') || f.contains('"') || f.contains('\n') || f.contains('\r');
    if !needs_quote {
        return f.to_string();
    }
    let replaced = f.replace('"', "\"\"");
    if starts_dangerous {
        format!("\"'{replaced}\"")
    } else {
        format!("\"{replaced}\"")
    }
}

/// GET /audit.csv — SuperAdmin-only. Same filters as `/audit.json` but capped
/// at 50k rows per request. Returns `text/csv; charset=utf-8`.
pub async fn csv(
    State(st): State<Arc<AppState>>,
    headers:   HeaderMap,
    Query(q):  Query<AuditQuery>,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return r; }
    let Some(pool) = st.db.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "database unavailable").into_response();
    };
    let limit = q.limit.unwrap_or(5000).clamp(1, 50_000);
    let prefix_like = q.action_prefix.as_ref().map(|p| format!("{p}%"));
    let (since, until) = resolve_window(&q);

    let rows_res = sqlx::query(
        r#"SELECT id, created_at, tenant_id, user_id, action, resource, status, metadata
             FROM audit_log
            WHERE ($1::uuid        IS NULL OR tenant_id  = $1)
              AND ($2::text        IS NULL OR action     LIKE $2)
              AND ($3::timestamptz IS NULL OR created_at >= $3)
              AND ($4::timestamptz IS NULL OR created_at <  $4)
              AND ($5::bigint      IS NULL OR id         <  $5)
            ORDER BY id DESC
            LIMIT $6"#,
    )
    .bind(q.tenant_id)
    .bind(prefix_like.as_deref())
    .bind(since)
    .bind(until)
    .bind(q.before_id)
    .bind(limit)
    .fetch_all(pool).await;

    let rows = match rows_res {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")).into_response(),
    };

    let mut buf = String::with_capacity(rows.len() * 128);
    buf.push_str("id,created_at,tenant_id,user_id,action,resource,status,metadata\r\n");
    for r in rows {
        let id:         i64 = r.try_get("id").unwrap_or_default();
        let created_at: OffsetDateTime = r.try_get("created_at").unwrap_or(OffsetDateTime::UNIX_EPOCH);
        let tenant_id:  uuid::Uuid = r.try_get("tenant_id").unwrap_or_else(|_| uuid::Uuid::nil());
        let user_id:    Option<uuid::Uuid> = r.try_get("user_id").ok().flatten();
        let action:     String = r.try_get("action").unwrap_or_default();
        let resource:   Option<String> = r.try_get("resource").ok().flatten();
        let status:     String = r.try_get("status").unwrap_or_else(|_| "unknown".into());
        let metadata:   serde_json::Value = r.try_get("metadata").unwrap_or(serde_json::json!({}));

        let created_at_str = created_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        let metadata_str = serde_json::to_string(&metadata).unwrap_or_default();

        buf.push_str(&csv_escape(&id.to_string()));
        buf.push(',');
        buf.push_str(&csv_escape(&created_at_str));
        buf.push(',');
        buf.push_str(&csv_escape(&tenant_id.to_string()));
        buf.push(',');
        buf.push_str(&csv_escape(&user_id.map(|u| u.to_string()).unwrap_or_default()));
        buf.push(',');
        buf.push_str(&csv_escape(&action));
        buf.push(',');
        buf.push_str(&csv_escape(&resource.unwrap_or_default()));
        buf.push(',');
        buf.push_str(&csv_escape(&status));
        buf.push(',');
        buf.push_str(&csv_escape(&metadata_str));
        buf.push_str("\r\n");
    }

    let filename = format!(
        "audit-{}.csv",
        OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "export".into())
    );
    (
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\"")),
        ],
        buf,
    ).into_response()
}

#[cfg(test)]
mod tests {
    use super::csv_escape;

    #[test]
    fn plain_field_unchanged() {
        assert_eq!(csv_escape("hello"), "hello");
        assert_eq!(csv_escape("user@example.com"), "user@example.com");
    }

    #[test]
    fn rfc4180_quoting_preserved() {
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("she said \"hi\""), "\"she said \"\"hi\"\"\"");
        assert_eq!(csv_escape("line1\nline2"), "\"line1\nline2\"");
    }

    #[test]
    fn formula_prefix_neutralized() {
        // Cada char perigoso vira string literal via prefixo `'` dentro de
        // field quoted — Excel/Sheets não avaliam.
        assert_eq!(csv_escape("=1+1"),         "\"'=1+1\"");
        assert_eq!(csv_escape("+CMD()"),       "\"'+CMD()\"");
        assert_eq!(csv_escape("-2+3"),         "\"'-2+3\"");
        assert_eq!(csv_escape("@SUM(A1)"),     "\"'@SUM(A1)\"");
        assert_eq!(csv_escape("\tinjected"),   "\"'\tinjected\"");
        assert_eq!(csv_escape("\rinjected"),   "\"'\rinjected\"");
    }

    #[test]
    fn formula_prefix_with_embedded_quote() {
        // Combina guard + escape de aspas (RFC 4180 doubling).
        assert_eq!(csv_escape("=HYPERLINK(\"x\")"),
                   "\"'=HYPERLINK(\"\"x\"\")\"");
    }

    #[test]
    fn dangerous_char_only_at_start() {
        // `=` no meio do campo é inofensivo (Excel só avalia se for o
        // primeiro char) — não precisa de prefixo, mas comma força quote.
        assert_eq!(csv_escape("a=b"),   "a=b");
        assert_eq!(csv_escape("a,=b"),  "\"a,=b\"");
        // E-mail com @ no meio: comum, não-perigoso.
        assert_eq!(csv_escape("u@d.com"), "u@d.com");
    }

    #[test]
    fn empty_field() {
        assert_eq!(csv_escape(""), "");
    }
}

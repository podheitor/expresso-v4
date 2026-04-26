//! expresso-compliance — retention policies, archival, and eDiscovery.
//!
//! # Retention enforcement
//!
//! A background task runs every `RETENTION_CHECK_INTERVAL_SECS` (default 3600s)
//! and for each enabled retention policy calls the expresso-mail bulk API to
//! hard-delete messages older than `retain_days` in the target folder(s).
//!
//! # Archive
//!
//! POST /internal/archive — called by expresso-mail (fire-and-forget) to
//! journal a copy of a delivered message into `compliance_archive`. The
//! body_path is stored as-is (S3 or filesystem path from the original store).
//!
//! # REST
//!
//! GET/POST/PATCH/DELETE /api/v1/compliance/retention-policies  (JWT auth, tenant-scoped)
//! GET             /api/v1/compliance/retention-policies/:id    (JWT auth, tenant-scoped)
//! GET             /api/v1/compliance/archive             (JWT auth, tenant-scoped; ?since=&before= date filters; ?subject=&from_addr=&to_addr= ILIKE; keyset pagination via before_id/after_id; ?size_min=&size_max=)
//! GET             /api/v1/compliance/archive/:id         (JWT auth, tenant-scoped)
//! DELETE          /api/v1/compliance/archive/:id         (JWT auth, tenant-scoped; GDPR/legal hold removal)
//!
//! Port: :8009

use std::{env, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    async_trait,
    extract::{FromRequestParts, Path, Query, Request, State},
    http::{header, request::Parts, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use expresso_auth_client::{AuthContext, Authenticated, AuthRejection, OidcConfig, OidcValidator};
use expresso_core::{begin_tenant_tx, create_db_pool, init_tracing, run_migrations, AppConfig};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};
use uuid::Uuid;

const SERVICE:                     &str = "expresso-compliance";
const DEFAULT_PORT:                 u16 = 8009;
const DEFAULT_RETENTION_INTERVAL:   u64 = 3600; // seconds

// ─── App state ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    db:        expresso_core::DbPool,
    mail_url:  String,   // expresso-mail base URL for bulk delete
    validator: Option<Arc<OidcValidator>>,
}

// ─── DTOs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct RetentionPolicy {
    pub id:          Uuid,
    pub tenant_id:   Uuid,
    pub folder_name: Option<String>,
    pub retain_days: i32,
    pub action:      String,
    pub enabled:     bool,
}

#[derive(Debug, sqlx::FromRow)]
struct RetentionPolicyDetail {
    pub id:          Uuid,
    pub tenant_id:   Uuid,
    pub folder_name: Option<String>,
    pub retain_days: i32,
    pub action:      String,
    pub enabled:     bool,
    pub created_at:  time::OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct CreatePolicyRequest {
    pub folder_name: Option<String>,
    pub retain_days: i32,
    pub action:      Option<String>,
    pub enabled:     Option<bool>,
}

#[derive(Debug, Deserialize)]
struct UpdatePolicyRequest {
    pub retain_days: Option<i32>,
    pub action:      Option<String>,
    pub enabled:     Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct ArchiveEntry {
    pub id:          Uuid,
    pub tenant_id:   Uuid,
    pub user_id:     Uuid,
    pub original_id: Option<Uuid>,
    pub body_path:   String,
    pub from_addr:   Option<String>,
    pub to_addrs:    serde_json::Value,
    pub subject:     Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub archived_at: time::OffsetDateTime,
    pub size_bytes:  i32,
}

/// Payload from expresso-mail for journaling.
#[derive(Debug, Deserialize)]
struct ArchiveRequest {
    pub tenant_id:   Uuid,
    pub user_id:     Uuid,
    pub message_id:  Option<Uuid>,
    pub body_path:   String,
    pub from_addr:   Option<String>,
    pub to_addrs:    Option<serde_json::Value>,
    pub subject:     Option<String>,
    pub size_bytes:  Option<i32>,
}

#[derive(Debug, Deserialize)]
struct ArchiveListParams {
    pub limit:     Option<i64>,
    /// Legacy offset (ignored when before_id/after_id is set).
    pub offset:    Option<i64>,
    /// ISO-8601 date prefix (YYYY-MM-DD) — entries archived on or after this date.
    pub since:     Option<String>,
    /// ISO-8601 date prefix (YYYY-MM-DD) — entries archived strictly before this date.
    pub before:    Option<String>,
    /// Keyset cursor — entries archived strictly before this entry (DESC order, next page).
    pub before_id: Option<Uuid>,
    /// Keyset cursor — entries archived strictly after this entry (ASC, then reversed).
    pub after_id:  Option<Uuid>,
    /// ILIKE filter on subject field.
    pub subject:   Option<String>,
    /// ILIKE filter on from_addr field.
    pub from_addr: Option<String>,
    /// ILIKE filter on any element in the to_addrs JSON array.
    pub to_addr:   Option<String>,
    /// Return only entries with size_bytes >= this value.
    pub size_min:  Option<i32>,
    /// Return only entries with size_bytes <= this value.
    pub size_max:  Option<i32>,
    /// Sort order for offset pagination: "asc" or "desc" (default "desc").
    /// Ignored when keyset cursors (before_id/after_id) are used.
    pub sort:      Option<String>,
}

// ─── Auth ─────────────────────────────────────────────────────────────────────

struct AuthCtx(AuthContext);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for AuthCtx {
    type Rejection = AuthRejection;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Authenticated(ctx) = Authenticated::from_request_parts(parts, state).await?;
        Ok(AuthCtx(ctx))
    }
}

async fn inject_validator(
    State(st): State<AppState>,
    mut req:   Request,
    next:      Next,
) -> Response {
    if let Some(v) = &st.validator {
        req.extensions_mut().insert(v.clone());
    }
    next.run(req).await
}

// ─── Retention policy CRUD ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ListPoliciesParams {
    /// Sort order: "asc" or "desc" (default "asc").
    sort: Option<String>,
}

async fn list_policies(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Query(params): Query<ListPoliciesParams>,
) -> Result<Json<Vec<RetentionPolicy>>, (StatusCode, Json<serde_json::Value>)> {
    let order = if params.sort.as_deref().map(|s| s.eq_ignore_ascii_case("desc")).unwrap_or(false) {
        "DESC"
    } else {
        "ASC"
    };
    let sql = format!(
        "SELECT id, tenant_id, folder_name, retain_days, action, enabled \
         FROM retention_policies \
         WHERE tenant_id = $1 \
         ORDER BY created_at {order}"
    );
    let rows: Vec<RetentionPolicy> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .fetch_all(&st.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(rows))
}

async fn create_policy(
    State(st):   State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Json(req):   Json<CreatePolicyRequest>,
) -> Result<(StatusCode, Json<RetentionPolicy>), (StatusCode, Json<serde_json::Value>)> {
    if req.retain_days <= 0 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "retain_days must be > 0"}))));
    }

    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let policy: RetentionPolicy = sqlx::query_as(
        "INSERT INTO retention_policies (tenant_id, folder_name, retain_days, action, enabled) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, tenant_id, folder_name, retain_days, action, enabled",
    )
    .bind(ctx.tenant_id)
    .bind(req.folder_name)
    .bind(req.retain_days)
    .bind(req.action.unwrap_or_else(|| "delete".into()))
    .bind(req.enabled.unwrap_or(true))
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tx.commit().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok((StatusCode::CREATED, Json(policy)))
}

async fn get_policy(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id):     Path<Uuid>,
    req_headers:  axum::http::HeaderMap,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let row: Option<RetentionPolicyDetail> = sqlx::query_as(
        "SELECT id, tenant_id, folder_name, retain_days, action, enabled, created_at \
         FROM retention_policies \
         WHERE id = $1 AND tenant_id = $2",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let p = row.ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({"error": "policy not found"}))))?;

    let etag = format!("\"{}-{}\"", p.created_at.unix_timestamp(), p.id);
    let last_modified = p.created_at
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap_or_default();

    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = time::OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if p.created_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }

    let policy = RetentionPolicy {
        id:          p.id,
        tenant_id:   p.tenant_id,
        folder_name: p.folder_name,
        retain_days: p.retain_days,
        action:      p.action,
        enabled:     p.enabled,
    };
    Ok((
        StatusCode::OK,
        [
            (header::ETAG,          etag),
            (header::LAST_MODIFIED, last_modified),
        ],
        Json(policy),
    ).into_response())
}

async fn update_policy(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id):     Path<Uuid>,
    Json(req):    Json<UpdatePolicyRequest>,
) -> Result<Json<RetentionPolicy>, (StatusCode, Json<serde_json::Value>)> {
    if let Some(d) = req.retain_days {
        if d <= 0 {
            return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "retain_days must be > 0"}))));
        }
    }

    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let policy: Option<RetentionPolicy> = sqlx::query_as(
        "UPDATE retention_policies \
         SET retain_days = COALESCE($3, retain_days), \
             action      = COALESCE($4, action), \
             enabled     = COALESCE($5, enabled), \
             updated_at  = NOW() \
         WHERE id = $1 AND tenant_id = $2 \
         RETURNING id, tenant_id, folder_name, retain_days, action, enabled",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(req.retain_days)
    .bind(req.action)
    .bind(req.enabled)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tx.commit().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    match policy {
        Some(p) => Ok(Json(p)),
        None    => Err((StatusCode::NOT_FOUND, Json(json!({"error": "policy not found"})))),
    }
}

async fn delete_policy(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id):     Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    sqlx::query("DELETE FROM retention_policies WHERE id = $1 AND tenant_id = $2")
        .bind(id)
        .bind(ctx.tenant_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tx.commit().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Archive ──────────────────────────────────────────────────────────────────

async fn archive_message(
    State(st):   State<AppState>,
    Json(req):   Json<ArchiveRequest>,
) -> Json<serde_json::Value> {
    let result = sqlx::query(
        "INSERT INTO compliance_archive \
           (tenant_id, user_id, original_id, body_path, from_addr, to_addrs, subject, size_bytes) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(req.tenant_id)
    .bind(req.user_id)
    .bind(req.message_id)
    .bind(&req.body_path)
    .bind(&req.from_addr)
    .bind(req.to_addrs.as_ref().unwrap_or(&json!([])))
    .bind(&req.subject)
    .bind(req.size_bytes.unwrap_or(0))
    .execute(&st.db)
    .await;

    match result {
        Ok(_)  => Json(json!({"ok": true})),
        Err(e) => { warn!(error = %e, "archive insert failed"); Json(json!({"ok": false})) }
    }
}

async fn list_archive(
    State(st):     State<AppState>,
    AuthCtx(ctx):  AuthCtx,
    Query(params): Query<ArchiveListParams>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.limit.unwrap_or(50).min(200);
    let order = if params.sort.as_deref().map(|s| s.eq_ignore_ascii_case("asc")).unwrap_or(false) {
        "ASC"
    } else {
        "DESC"
    };

    let since_filter = params.since
        .map(|d| format!("AND archived_at >= '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let before_date_filter = params.before
        .map(|d| format!("AND archived_at < '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let subject_filter = params.subject.map(|s| {
        let esc = s.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
        format!("AND subject ILIKE '%{esc}%'")
    }).unwrap_or_default();
    let from_addr_filter = params.from_addr.map(|f| {
        let esc = f.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
        format!("AND from_addr ILIKE '%{esc}%'")
    }).unwrap_or_default();
    let to_addr_filter = params.to_addr.map(|t| {
        let esc = t.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
        format!("AND EXISTS (SELECT 1 FROM jsonb_array_elements_text(to_addrs) t WHERE t ILIKE '%{esc}%')")
    }).unwrap_or_default();
    let size_min_filter = params.size_min
        .map(|v| format!("AND size_bytes >= {v}"))
        .unwrap_or_default();
    let size_max_filter = params.size_max
        .map(|v| format!("AND size_bytes <= {v}"))
        .unwrap_or_default();

    let base =
        "SELECT id, tenant_id, user_id, original_id, body_path, from_addr, \
                to_addrs, subject, archived_at, size_bytes \
         FROM compliance_archive \
         WHERE tenant_id = $1 AND user_id = $2";

    let rows: Vec<ArchiveEntry> = if let Some(cursor_id) = params.before_id.or(params.after_id) {
        let is_before = params.before_id.is_some();

        let anchor: Option<(time::OffsetDateTime, Uuid)> = sqlx::query_as(
            "SELECT archived_at, id FROM compliance_archive \
             WHERE id = $1 AND tenant_id = $2 AND user_id = $3 LIMIT 1",
        )
        .bind(cursor_id)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_optional(&st.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

        let (anchor_ts, anchor_id) = anchor.ok_or_else(|| {
            (StatusCode::NOT_FOUND, Json(json!({"error": "cursor entry not found"})))
        })?;

        if is_before {
            let sql = format!(
                "{base} {since_filter} {before_date_filter} {subject_filter} {from_addr_filter} \
                 {to_addr_filter} {size_min_filter} {size_max_filter} \
                 AND (archived_at, id) < ($4::timestamptz, $5::uuid) \
                 ORDER BY archived_at DESC, id DESC LIMIT {limit}"
            );
            sqlx::query_as(&sql)
                .bind(ctx.tenant_id)
                .bind(ctx.user_id)
                .bind(anchor_ts)
                .bind(anchor_id)
                .fetch_all(&st.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        } else {
            let sql = format!(
                "{base} {since_filter} {before_date_filter} {subject_filter} {from_addr_filter} \
                 {to_addr_filter} {size_min_filter} {size_max_filter} \
                 AND (archived_at, id) > ($4::timestamptz, $5::uuid) \
                 ORDER BY archived_at ASC, id ASC LIMIT {limit}"
            );
            let mut rows: Vec<ArchiveEntry> = sqlx::query_as(&sql)
                .bind(ctx.tenant_id)
                .bind(ctx.user_id)
                .bind(anchor_ts)
                .bind(anchor_id)
                .fetch_all(&st.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
            rows.reverse();
            rows
        }
    } else {
        let offset = params.offset.unwrap_or(0);
        let sql = format!(
            "{base} {since_filter} {before_date_filter} {subject_filter} {from_addr_filter} \
             {to_addr_filter} {size_min_filter} {size_max_filter} \
             ORDER BY archived_at {order}, id {order} LIMIT {limit} OFFSET {offset}"
        );
        sqlx::query_as(&sql)
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .fetch_all(&st.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
    };

    // Count total matching rows (same filters, no pagination) for X-Total-Count header.
    let count_sql = format!(
        "SELECT COUNT(*) FROM compliance_archive \
         WHERE tenant_id = $1 AND user_id = $2 \
         {since_filter} {before_date_filter} {subject_filter} {from_addr_filter} \
         {to_addr_filter} {size_min_filter} {size_max_filter}"
    );
    let total: i64 = sqlx::query_scalar(&count_sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_one(&st.db)
        .await
        .unwrap_or(0);

    Ok((
        StatusCode::OK,
        [(header::HeaderName::from_static("x-total-count"), total.to_string())],
        Json(rows),
    ).into_response())
}

/// GET /api/v1/compliance/archive/:id — fetch a single archived entry by ID.
/// Returns ETag (`"{archived_at_unix}-{id}"`) and Last-Modified. Responds 304 if If-None-Match matches.
async fn get_archive_entry(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id):     Path<Uuid>,
    req_headers:  axum::http::HeaderMap,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let row: Option<ArchiveEntry> = sqlx::query_as(
        "SELECT id, tenant_id, user_id, original_id, body_path, from_addr, \
                to_addrs, subject, archived_at, size_bytes \
         FROM compliance_archive \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let entry = row.ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({"error": "archive entry not found"}))))?;

    let etag = format!("\"{}-{}\"", entry.archived_at.unix_timestamp(), entry.id);
    let last_modified = entry.archived_at
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap_or_default();

    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }

    Ok((
        StatusCode::OK,
        [
            (header::ETAG,          etag),
            (header::LAST_MODIFIED, last_modified),
        ],
        Json(entry),
    ).into_response())
}

/// DELETE /api/v1/compliance/archive/:id — remove a single archived entry (GDPR/legal hold).
async fn delete_archive_entry(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id):     Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let result = sqlx::query(
        "DELETE FROM compliance_archive WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .execute(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    if result.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(json!({"error": "archive entry not found"}))));
    }

    Ok(StatusCode::NO_CONTENT)
}

// ─── Retention enforcement background task ────────────────────────────────────

async fn run_retention_loop(db: expresso_core::DbPool, mail_url: String, interval_secs: u64) {
    let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
    loop {
        ticker.tick().await;
        enforce_retention(&db, &mail_url).await;
    }
}

async fn enforce_retention(db: &expresso_core::DbPool, mail_url: &str) {
    if mail_url.is_empty() {
        return;
    }

    let policies: Vec<RetentionPolicy> = match sqlx::query_as(
        "SELECT id, tenant_id, folder_name, retain_days, action, enabled \
         FROM retention_policies \
         WHERE enabled = TRUE",
    )
    .fetch_all(db)
    .await {
        Ok(p)  => p,
        Err(e) => { warn!(error = %e, "retention: failed to fetch policies"); return; }
    };

    for policy in &policies {
        if policy.action != "delete" {
            continue;
        }

        // Fetch message IDs older than retain_days for this tenant+folder.
        let folder_clause = if let Some(folder) = &policy.folder_name {
            format!("AND mb.folder_name = '{}'", folder.replace('\'', "''"))
        } else {
            String::new()
        };

        let sql = format!(
            "SELECT m.id \
             FROM messages m \
             JOIN mailboxes mb ON mb.id = m.mailbox_id \
             WHERE m.tenant_id = $1 \
               AND m.received_at < NOW() - INTERVAL '{} days' \
               {folder_clause} \
             LIMIT 500",
            policy.retain_days
        );

        let ids: Vec<(Uuid,)> = match sqlx::query_as(&sql)
            .bind(policy.tenant_id)
            .fetch_all(db)
            .await
        {
            Ok(r)  => r,
            Err(e) => { warn!(error = %e, "retention: query failed"); continue; }
        };

        if ids.is_empty() {
            continue;
        }

        let id_list: Vec<Uuid> = ids.into_iter().map(|(id,)| id).collect();
        info!(
            tenant = %policy.tenant_id,
            count  = id_list.len(),
            folder = ?policy.folder_name,
            "retention: deleting expired messages"
        );

        // Call expresso-mail bulk delete (internal — no auth required).
        let payload = json!({
            "action": "delete",
            "ids":    id_list,
        });
        let url = format!("{mail_url}/api/v1/mail/messages/bulk");
        if let Err(e) = reqwest::Client::new()
            .post(&url)
            .json(&payload)
            .timeout(Duration::from_secs(10))
            .send()
            .await
        {
            warn!(error = %e, "retention: bulk delete call failed");
        }
    }
}

// ─── Misc ─────────────────────────────────────────────────────────────────────

async fn health() -> Json<serde_json::Value> {
    Json(json!({"service": SERVICE, "status": "ok"}))
}

async fn ready() -> Json<serde_json::Value> {
    Json(json!({"ready": true}))
}

async fn maybe_build_validator() -> Option<Arc<OidcValidator>> {
    let issuer   = env::var("AUTH__OIDC_ISSUER").ok().filter(|v| !v.is_empty())?;
    let audience = env::var("AUTH__OIDC_AUDIENCE").ok().filter(|v| !v.is_empty())?;
    let cfg = OidcConfig::new(issuer.clone(), audience);
    match OidcValidator::new(cfg).await {
        Ok(v)  => { info!(issuer = %issuer, "OIDC validator ready"); Some(Arc::new(v)) }
        Err(e) => { warn!(error = %e, "OIDC init failed — no JWT auth"); None }
    }
}

fn resolve_addr() -> anyhow::Result<SocketAddr> {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port  = env::var("PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    format!("{host}:{port}").parse::<SocketAddr>()
        .map_err(|e| anyhow::anyhow!("invalid bind address: {}", e))
}

// ─── Entrypoint ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = AppConfig::from_env()?;
    init_tracing(&cfg.telemetry);

    info!(version = env!("CARGO_PKG_VERSION"), "{SERVICE} starting");

    let db = create_db_pool(&cfg.database).await?;
    run_migrations(&db).await?;

    let mail_url = env::var("MAIL_URL").unwrap_or_default();
    let validator = maybe_build_validator().await;

    let state = AppState { db: db.clone(), mail_url: mail_url.clone(), validator };

    // Background retention enforcement.
    let interval = env::var("RETENTION_CHECK_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_RETENTION_INTERVAL);
    tokio::spawn(run_retention_loop(db, mail_url, interval));

    let app = Router::new()
        .route("/health",                              get(health))
        .route("/ready",                               get(ready))
        .route("/internal/archive",                    post(archive_message))
        .route("/api/v1/compliance/retention-policies",
               get(list_policies).post(create_policy))
        .route("/api/v1/compliance/retention-policies/:id",
               get(get_policy).patch(update_policy).delete(delete_policy))
        .route("/api/v1/compliance/archive",           get(list_archive))
        .route("/api/v1/compliance/archive/:id",       get(get_archive_entry).delete(delete_archive_entry))
        .merge(expresso_observability::metrics_router())
        .layer(middleware::from_fn_with_state(state.clone(), inject_validator))
        .with_state(state);

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(service = SERVICE, %addr, "listening");
    axum::serve(listener, app).await?;

    Ok(())
}

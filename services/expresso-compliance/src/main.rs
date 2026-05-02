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

use hmac::{Hmac, Mac};
use sha2::Sha256;
type HmacSha256 = Hmac<Sha256>;

use axum::{
    async_trait,
    extract::{FromRequestParts, Path, Query, Request, State},
    http::{header, request::Parts, HeaderMap, HeaderValue, StatusCode},
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
    pub updated_at:  time::OffsetDateTime,
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
    /// Optional AES-256 password to encrypt the exported ZIP.
    pub password:  Option<String>,
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
    State(st):      State<AppState>,
    AuthCtx(ctx):   AuthCtx,
    Query(params):  Query<ListPoliciesParams>,
    req_headers:    axum::http::HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let order = if params.sort.as_deref().map(|s| s.eq_ignore_ascii_case("desc")).unwrap_or(false) {
        "DESC"
    } else {
        "ASC"
    };

    let max_ts: Option<time::OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(updated_at) FROM retention_policies WHERE tenant_id = $1",
    )
    .bind(ctx.tenant_id)
    .fetch_one(&st.db)
    .await
    .unwrap_or(None);

    if let Some(ts) = max_ts {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = time::OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM retention_policies WHERE tenant_id = $1",
    )
    .bind(ctx.tenant_id)
    .fetch_one(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
    let sql = format!(
        "SELECT id, tenant_id, folder_name, retain_days, action, enabled \
         FROM retention_policies \
         WHERE tenant_id = $1 \
         ORDER BY updated_at {order}"
    );
    let rows: Vec<RetentionPolicy> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .fetch_all(&st.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let mut resp = (
        StatusCode::OK,
        [(header::HeaderName::from_static("x-total-count"), total.to_string())],
        Json(rows),
    ).into_response();
    if let Some(ts) = max_ts {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, axum::http::HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
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
        "SELECT id, tenant_id, folder_name, retain_days, action, enabled, updated_at \
         FROM retention_policies \
         WHERE id = $1 AND tenant_id = $2",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let p = row.ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({"error": "policy not found"}))))?;

    let etag = format!("\"{}-{}\"", p.updated_at.unix_timestamp(), p.id);
    let last_modified = p.updated_at
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
                if p.updated_at <= ims_dt {
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
    req_headers:   HeaderMap,
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

    let max_archived: Option<time::OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(archived_at) FROM compliance_archive WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(&st.db)
    .await
    .unwrap_or(None);

    if let Some(ts) = max_archived {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = time::OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

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

    let mut resp = (
        StatusCode::OK,
        [(header::HeaderName::from_static("x-total-count"), total.to_string())],
        Json(rows),
    ).into_response();
    if let Some(ts) = max_archived {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

/// GET /api/v1/compliance/archive/count — retorna apenas a contagem de entradas
/// no archive do usuário, com os mesmos filtros do list_archive (since, before,
/// subject, from_addr, to_addr, size_min, size_max), sem listar nem paginar
/// (sprint #425). Útil pra dashboards e badges sem custo de serializar payload.
async fn count_archive(
    State(st):     State<AppState>,
    AuthCtx(ctx):  AuthCtx,
    Query(params): Query<ArchiveListParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
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

    let sql = format!(
        "SELECT COUNT(*) FROM compliance_archive \
         WHERE tenant_id = $1 AND user_id = $2 \
         {since_filter} {before_date_filter} {subject_filter} {from_addr_filter} \
         {to_addr_filter} {size_min_filter} {size_max_filter}"
    );
    let count: i64 = sqlx::query_scalar(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_one(&st.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({"count": count})))
}

#[derive(Debug, Deserialize)]
struct ArchiveHistogramParams {
    /// ISO-8601 date prefix (YYYY-MM-DD) — bucket inferior inclusive.
    pub since:  Option<String>,
    /// ISO-8601 date prefix — bucket superior exclusive.
    pub before: Option<String>,
    /// "day" (default), "week", or "month". Whitelist evita injection no date_trunc.
    pub bucket: Option<String>,
}

/// GET /api/v1/compliance/archive/histogram?since=&before=&bucket=day — agrupa
/// entradas do archive por bucket temporal (sprint #435). Retorna `{bucket, series:
/// [{ts, count}]}` ordenado por ts ascendente. Bucket é whitelist (day/week/month);
/// usa `date_trunc()` no Postgres pra agrupar archived_at. Não exposto outros filtros
/// (subject/from_addr) — histogram é primariamente pra dashboards de volume temporal.
async fn histogram_archive(
    State(st):     State<AppState>,
    AuthCtx(ctx):  AuthCtx,
    Query(params): Query<ArchiveHistogramParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let bucket = match params.bucket.as_deref().unwrap_or("day") {
        "day"   => "day",
        "week"  => "week",
        "month" => "month",
        other   => return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("invalid bucket '{other}': expected day/week/month")})),
        )),
    };

    let since_filter = params.since
        .map(|d| format!("AND archived_at >= '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let before_filter = params.before
        .map(|d| format!("AND archived_at < '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();

    let sql = format!(
        "SELECT date_trunc('{bucket}', archived_at) AS ts, COUNT(*) AS c \
         FROM compliance_archive \
         WHERE tenant_id = $1 AND user_id = $2 \
         {since_filter} {before_filter} \
         GROUP BY ts ORDER BY ts ASC"
    );

    let rows: Vec<(time::OffsetDateTime, i64)> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_all(&st.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let series: Vec<_> = rows.into_iter()
        .map(|(ts, c)| json!({
            "ts":    ts.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
            "count": c,
        }))
        .collect();

    Ok(Json(json!({ "bucket": bucket, "series": series })))
}

#[derive(Debug, Deserialize)]
struct TopSendersParams {
    /// ISO-8601 date prefix (YYYY-MM-DD) — limite inferior inclusive.
    pub since:  Option<String>,
    /// ISO-8601 date prefix — limite superior exclusive.
    pub before: Option<String>,
    /// Top-N (default 10, cap 100). Sidebar de "top remetentes" não precisa
    /// mais que isso; cap protege payload contra `?limit=999999`.
    pub limit:  Option<i64>,
}

/// GET /api/v1/compliance/archive/top-senders?since=&before=&limit=10 — retorna
/// top-N remetentes mais frequentes no archive (sprint #440). Agrupa por from_addr,
/// conta entries, ordena DESC + alfabético. Path com hífen pra deixar espaço pra
/// `/archive/top-recipients` futuro sem colisão. Filtros since/before reusam mesmo
/// padrão de escape do count_archive (#425) e histogram (#435). Útil pra dashboards
/// de "quem mais arquivou", complementa histogram (volume temporal).
async fn top_senders_archive(
    State(st):     State<AppState>,
    AuthCtx(ctx):  AuthCtx,
    Query(params): Query<TopSendersParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.limit.unwrap_or(10).clamp(1, 100);

    let since_filter = params.since
        .map(|d| format!("AND archived_at >= '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let before_filter = params.before
        .map(|d| format!("AND archived_at < '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();

    let sql = format!(
        "SELECT COALESCE(from_addr, '(unknown)') AS sender, COUNT(*) AS c \
         FROM compliance_archive \
         WHERE tenant_id = $1 AND user_id = $2 \
         {since_filter} {before_filter} \
         GROUP BY sender ORDER BY c DESC, sender ASC LIMIT {limit}"
    );

    let rows: Vec<(String, i64)> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_all(&st.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let senders: Vec<_> = rows.into_iter()
        .map(|(sender, count)| json!({ "sender": sender, "count": count }))
        .collect();

    Ok(Json(json!({ "limit": limit, "senders": senders })))
}

/// GET /api/v1/compliance/archive/top-recipients?since=&before=&limit=10 — retorna
/// top-N destinatários mais frequentes no archive (sprint #445). Complementa
/// top-senders (#440): explode `to_addrs` (JSON array) com `jsonb_array_elements_text`
/// e agrupa por destinatário. Mesmo schema de filtros (since/before) e mesmo
/// padrão de escape via replace('\'', "''"). Útil pra "quem mais recebe" em
/// retention/legal-hold dashboards. Entries sem to_addrs ficam fora do count
/// (jsonb_array_elements_text de NULL ou '[]' não produz linhas).
async fn top_recipients_archive(
    State(st):     State<AppState>,
    AuthCtx(ctx):  AuthCtx,
    Query(params): Query<TopSendersParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.limit.unwrap_or(10).clamp(1, 100);

    let since_filter = params.since
        .map(|d| format!("AND archived_at >= '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let before_filter = params.before
        .map(|d| format!("AND archived_at < '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();

    let sql = format!(
        "SELECT t AS recipient, COUNT(*) AS c \
         FROM compliance_archive, \
              jsonb_array_elements_text(COALESCE(to_addrs, '[]'::jsonb)) AS t \
         WHERE tenant_id = $1 AND user_id = $2 \
         {since_filter} {before_filter} \
         GROUP BY recipient ORDER BY c DESC, recipient ASC LIMIT {limit}"
    );

    let rows: Vec<(String, i64)> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_all(&st.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let recipients: Vec<_> = rows.into_iter()
        .map(|(recipient, count)| json!({ "recipient": recipient, "count": count }))
        .collect();

    Ok(Json(json!({ "limit": limit, "recipients": recipients })))
}

/// GET /api/v1/compliance/archive/top-subjects?since=&before=&limit=10 — retorna
/// top-N subjects mais frequentes no archive (sprint #450). Complementa top-senders
/// (#440) e top-recipients (#445); GROUP BY subject direto, COALESCE pra NULL,
/// trim+lower pra normalizar (Re:/RE:/re: viram bucket único). Mesmo schema de
/// filtros (since/before) e mesmo padrão de escape via replace('\'', "''").
/// Útil pra "qual assunto domina o archive" — threads recorrentes, alertas
/// automatizados, newsletters em massa.
async fn top_subjects_archive(
    State(st):     State<AppState>,
    AuthCtx(ctx):  AuthCtx,
    Query(params): Query<TopSendersParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.limit.unwrap_or(10).clamp(1, 100);

    let since_filter = params.since
        .map(|d| format!("AND archived_at >= '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let before_filter = params.before
        .map(|d| format!("AND archived_at < '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();

    let sql = format!(
        "SELECT LOWER(TRIM(COALESCE(subject, '(no subject)'))) AS subj, COUNT(*) AS c \
         FROM compliance_archive \
         WHERE tenant_id = $1 AND user_id = $2 \
         {since_filter} {before_filter} \
         GROUP BY subj ORDER BY c DESC, subj ASC LIMIT {limit}"
    );

    let rows: Vec<(String, i64)> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_all(&st.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let subjects: Vec<_> = rows.into_iter()
        .map(|(subject, count)| json!({ "subject": subject, "count": count }))
        .collect();

    Ok(Json(json!({ "limit": limit, "subjects": subjects })))
}

/// GET /api/v1/compliance/archive/top-domains?since=&before=&limit=10 — retorna
/// top-N domínios remetentes mais frequentes no archive (sprint #451). Paralelo SQL
/// do facet `domain` em search (#441) e cross `domain_x_kind` (#446): extrai parte
/// after-@ do from_addr via `split_part(from_addr, '@', 2)`, normaliza com LOWER.
/// Entries sem '@' (from_addr legacy ou malformado) caem no bucket '(unknown)'.
/// Mesmo schema de filtros (since/before) e padrão de escape via replace('\'', "''").
/// Útil pra "quais domínios dominam o tráfego arquivado" — detecta vendors,
/// newsletters massivas, parceiros B2B recorrentes.
async fn top_domains_archive(
    State(st):     State<AppState>,
    AuthCtx(ctx):  AuthCtx,
    Query(params): Query<TopSendersParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.limit.unwrap_or(10).clamp(1, 100);

    let since_filter = params.since
        .map(|d| format!("AND archived_at >= '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let before_filter = params.before
        .map(|d| format!("AND archived_at < '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();

    let sql = format!(
        "SELECT CASE \
            WHEN from_addr IS NULL OR position('@' in from_addr) = 0 THEN '(unknown)' \
            ELSE LOWER(split_part(from_addr, '@', 2)) \
         END AS domain, COUNT(*) AS c \
         FROM compliance_archive \
         WHERE tenant_id = $1 AND user_id = $2 \
         {since_filter} {before_filter} \
         GROUP BY domain ORDER BY c DESC, domain ASC LIMIT {limit}"
    );

    let rows: Vec<(String, i64)> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_all(&st.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let domains: Vec<_> = rows.into_iter()
        .map(|(domain, count)| json!({ "domain": domain, "count": count }))
        .collect();

    Ok(Json(json!({ "limit": limit, "domains": domains })))
}

/// GET /api/v1/compliance/archive/size-histogram?since=&before= — distribuição
/// de tamanho de mensagens arquivadas em buckets de bytes (sprint #456). Retorna
/// `{buckets: [{bucket, min_bytes, max_bytes, count}]}` em ordem crescente.
/// Buckets fixos cobrem espectro típico de email: <1KB, 1-10KB, 10-100KB,
/// 100KB-1MB, 1-10MB, 10-25MB, >25MB. Use `width_bucket` do Postgres com
/// thresholds explícitos pra classificação O(log N) por linha. Mesmo schema
/// since/before do histogram temporal (#435). Path com hífen evita colisão com
/// `/archive/:id` (lição #443/#448 — rotas estáticas precedem `:id`, mas mantemos
/// hífen porque `size-histogram` tem dois segmentos lógicos).
async fn size_histogram_archive(
    State(st):     State<AppState>,
    AuthCtx(ctx):  AuthCtx,
    Query(params): Query<TopSendersParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let since_filter = params.since
        .map(|d| format!("AND archived_at >= '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let before_filter = params.before
        .map(|d| format!("AND archived_at < '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();

    // Thresholds em bytes (exclusivo no upper). width_bucket(v, lo, hi, n) retorna
    // 0 quando v < lo, n+1 quando v >= hi; usamos thresholds custom via CASE.
    let sql = format!(
        "SELECT bucket, COUNT(*) AS c FROM ( \
            SELECT CASE \
                WHEN size_bytes <       1024 THEN 0 \
                WHEN size_bytes <      10240 THEN 1 \
                WHEN size_bytes <     102400 THEN 2 \
                WHEN size_bytes <    1048576 THEN 3 \
                WHEN size_bytes <   10485760 THEN 4 \
                WHEN size_bytes <   26214400 THEN 5 \
                ELSE                              6 \
            END AS bucket \
            FROM compliance_archive \
            WHERE tenant_id = $1 AND user_id = $2 \
            {since_filter} {before_filter} \
         ) sub GROUP BY bucket ORDER BY bucket ASC"
    );

    let rows: Vec<(i32, i64)> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_all(&st.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let labels: [(&str, i64, Option<i64>); 7] = [
        ("<1KB",        0,         Some(1024)),
        ("1-10KB",      1024,      Some(10240)),
        ("10-100KB",    10240,     Some(102400)),
        ("100KB-1MB",   102400,    Some(1048576)),
        ("1-10MB",      1048576,   Some(10485760)),
        ("10-25MB",     10485760,  Some(26214400)),
        (">25MB",       26214400,  None),
    ];

    // Materializa todos os buckets (mesmo zero) pra UI estável.
    let mut counts = [0i64; 7];
    for (b, c) in rows { if (0..7).contains(&b) { counts[b as usize] = c; } }

    let buckets: Vec<_> = labels.iter().enumerate()
        .map(|(i, (label, lo, hi))| json!({
            "bucket":    label,
            "min_bytes": lo,
            "max_bytes": hi,
            "count":     counts[i],
        }))
        .collect();

    Ok(Json(json!({ "buckets": buckets })))
}

/// GET /api/v1/compliance/archive/top-tags?limit=10&since=&before= — top tags
/// usadas em archive entries do user (sprint #461). Conta por tag via JOIN
/// compliance_archive_tags + compliance_archive (filtra por tenant + user_id +
/// archived_at range). Retorna `{limit, tags: [{tag, count}]}`. Útil pra
/// dashboard de e-discovery — quais case-IDs/hold-tags concentram volume.
async fn top_tags_archive(
    State(st):     State<AppState>,
    AuthCtx(ctx):  AuthCtx,
    Query(params): Query<TopSendersParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.limit.unwrap_or(10).clamp(1, 100);

    let since_filter = params.since
        .map(|d| format!("AND a.archived_at >= '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let before_filter = params.before
        .map(|d| format!("AND a.archived_at < '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();

    let sql = format!(
        "SELECT t.tag, COUNT(*) AS c \
         FROM compliance_archive_tags t \
         JOIN compliance_archive a ON a.id = t.archive_id AND a.tenant_id = t.tenant_id \
         WHERE t.tenant_id = $1 AND a.user_id = $2 \
         {since_filter} {before_filter} \
         GROUP BY t.tag ORDER BY c DESC, t.tag ASC LIMIT {limit}"
    );

    let rows: Vec<(String, i64)> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_all(&st.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let tags: Vec<_> = rows.into_iter()
        .map(|(tag, count)| json!({ "tag": tag, "count": count }))
        .collect();

    Ok(Json(json!({ "limit": limit, "tags": tags })))
}

/// GET /api/v1/compliance/archive/tags/intersect?tags=a,b,c — lista archive
/// entries do user que possuem TODAS as tags listadas (AND semantic, sprint
/// #466). Paralelo do drive intersect (#465). Tags normalizadas lowercase +
/// trim, deduped, 1-32 por query, 1-64 chars cada. Query AND-set canônica:
/// `WHERE t.tag = ANY($3::text[]) + GROUP BY t.archive_id HAVING COUNT(DISTINCT t.tag) = N`
/// (evita N self-joins). Retorna entries completos via JOIN. Static
/// `/tags/intersect` precede `/tags/:tag`.
#[derive(Debug, Deserialize)]
struct ArchiveIntersectQuery {
    tags: String,
}

async fn archive_entries_intersect(
    State(st):     State<AppState>,
    AuthCtx(ctx):  AuthCtx,
    Query(params): Query<ArchiveIntersectQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mut tags: Vec<String> = params
        .tags
        .split(',')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    if tags.is_empty() || tags.len() > 32 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "tags must be 1..32 entries"}))));
    }
    if tags.iter().any(|t| t.chars().count() > 64) {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "each tag must be 1-64 characters"}))));
    }
    let n = tags.len() as i64;

    let entries: Vec<ArchiveEntry> = sqlx::query_as(
        "SELECT a.id, a.tenant_id, a.user_id, a.original_id, a.body_path, a.from_addr, \
                a.to_addrs, a.subject, a.archived_at, a.size_bytes \
         FROM compliance_archive a \
         JOIN compliance_archive_tags t ON t.archive_id = a.id AND t.tenant_id = a.tenant_id \
         WHERE a.tenant_id = $1 AND a.user_id = $2 AND t.tag = ANY($3::text[]) \
         GROUP BY a.id \
         HAVING COUNT(DISTINCT t.tag) = $4 \
         ORDER BY a.archived_at DESC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&tags)
    .bind(n)
    .fetch_all(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({ "tags": tags, "entries": entries })))
}

/// GET /api/v1/compliance/archive/tags/union?tags=a,b,c — lista archive
/// entries do user que possuem PELO MENOS UMA das tags listadas (OR semantic,
/// sprint #471). Paralelo do drive union (#467) e complemento ao intersect
/// (#466, AND). Reusa `ArchiveIntersectQuery` (mesma normalização). Query
/// OR-set: `WHERE t.tag = ANY($3::text[]) + GROUP BY a.id` (sem HAVING — basta
/// match em qualquer tag). Static `/tags/union` precede `/tags/:tag`.
async fn archive_entries_union(
    State(st):     State<AppState>,
    AuthCtx(ctx):  AuthCtx,
    Query(params): Query<ArchiveIntersectQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mut tags: Vec<String> = params
        .tags
        .split(',')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    if tags.is_empty() || tags.len() > 32 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "tags must be 1..32 entries"}))));
    }
    if tags.iter().any(|t| t.chars().count() > 64) {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "each tag must be 1-64 characters"}))));
    }

    let entries: Vec<ArchiveEntry> = sqlx::query_as(
        "SELECT a.id, a.tenant_id, a.user_id, a.original_id, a.body_path, a.from_addr, \
                a.to_addrs, a.subject, a.archived_at, a.size_bytes \
         FROM compliance_archive a \
         JOIN compliance_archive_tags t ON t.archive_id = a.id AND t.tenant_id = a.tenant_id \
         WHERE a.tenant_id = $1 AND a.user_id = $2 AND t.tag = ANY($3::text[]) \
         GROUP BY a.id \
         ORDER BY a.archived_at DESC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&tags)
    .fetch_all(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({ "tags": tags, "entries": entries })))
}

/// GET /api/v1/compliance/archive/tags/:tag — lista archive entries que
/// possuem a tag (sprint #462). Tag normalizada lowercase pra match com
/// add_archive_tag. Retorna `{tag, entries: [...]}` ordenado por archived_at
/// DESC. Static `/tags` precede `/:id` em axum (lição #443/#448) e `:tag` final
/// é distinto de `:id` (UUID) — sem colisão. Complementa top-tags (#461)
/// permitindo drill-down por rótulo.
async fn archive_entries_by_tag(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(tag):    Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let tag = tag.trim().to_lowercase();
    if tag.is_empty() || tag.chars().count() > 64 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "tag must be 1-64 characters"}))));
    }

    let entries: Vec<ArchiveEntry> = sqlx::query_as(
        "SELECT a.id, a.tenant_id, a.user_id, a.original_id, a.body_path, a.from_addr, \
                a.to_addrs, a.subject, a.archived_at, a.size_bytes \
         FROM compliance_archive a \
         JOIN compliance_archive_tags t ON t.archive_id = a.id AND t.tenant_id = a.tenant_id \
         WHERE a.tenant_id = $1 AND a.user_id = $2 AND t.tag = $3 \
         ORDER BY a.archived_at DESC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&tag)
    .fetch_all(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({ "tag": tag, "entries": entries })))
}

#[derive(Debug, Deserialize)]
struct RenameArchiveTagBody {
    new_tag: String,
}

/// PATCH /api/v1/compliance/archive/tags/:tag — renomeia uma tag em todos os
/// archive entries do user no tenant (sprint #475). Paralelo ao drive rename
/// (#430+#470). Body: `{new_tag: "..."}`. Pré-DELETE de conflitos + UPDATE +
/// INSERT em compliance_archive_tag_rename_history numa única transação via
/// `begin_tenant_tx` (RLS) — atomicidade garantida. History grava `{tenant_id,
/// user_id, old_tag, new_tag, renamed_count, renamed_by, renamed_at}` para
/// audit trail e undo manual. Rename é escopado a `user_id` (archive tags são
/// user-scoped por design — cada user gerencia seus próprios rótulos).
async fn rename_archive_tag(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(tag):    Path<String>,
    Json(body):   Json<RenameArchiveTagBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let old = tag.trim().to_lowercase();
    let new = body.new_tag.trim().to_lowercase();
    if old.is_empty() || old.chars().count() > 64 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "tag must be 1-64 characters"}))));
    }
    if new.is_empty() || new.chars().count() > 64 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "new_tag must be 1-64 characters"}))));
    }
    if new == old {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "new_tag must differ from old tag"}))));
    }

    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    // Apaga registros que já tinham new_tag nos archives que também têm old_tag.
    let _ = sqlx::query(
        "DELETE FROM compliance_archive_tags \
         WHERE tenant_id = $1 AND tag = $2 \
           AND archive_id IN ( \
               SELECT t.archive_id FROM compliance_archive_tags t \
               JOIN compliance_archive a ON a.id = t.archive_id AND a.tenant_id = t.tenant_id \
               WHERE t.tenant_id = $1 AND t.tag = $3 AND a.user_id = $4 \
           )",
    )
    .bind(ctx.tenant_id)
    .bind(&new)
    .bind(&old)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let r = sqlx::query(
        "UPDATE compliance_archive_tags SET tag = $2 \
         WHERE tenant_id = $1 AND tag = $3 \
           AND archive_id IN ( \
               SELECT id FROM compliance_archive \
               WHERE tenant_id = $1 AND user_id = $4 \
           )",
    )
    .bind(ctx.tenant_id)
    .bind(&new)
    .bind(&old)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let renamed = r.rows_affected();

    let history_id: (Uuid,) = sqlx::query_as(
        "INSERT INTO compliance_archive_tag_rename_history \
            (tenant_id, user_id, old_tag, new_tag, renamed_count, renamed_by) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&old)
    .bind(&new)
    .bind(renamed as i64)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({
        "renamed":    renamed,
        "old_tag":    old,
        "new_tag":    new,
        "history_id": history_id.0,
    })))
}

#[derive(Debug, Deserialize)]
struct ArchiveTagRenameHistoryQuery {
    limit:  Option<i64>,
    since:  Option<time::OffsetDateTime>,
    before: Option<time::OffsetDateTime>,
    tag:    Option<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ArchiveTagRenameHistoryEntry {
    id:            Uuid,
    old_tag:       String,
    new_tag:       String,
    renamed_count: i64,
    renamed_by:    Uuid,
    #[serde(with = "time::serde::rfc3339")]
    renamed_at:    time::OffsetDateTime,
}

/// GET /api/v1/compliance/archive/tags/rename-history?limit=&since=&before=&tag=
/// — audit trail dos renames de tag passados pelo user (sprint #475). Filtros
/// opcionais: range temporal e `tag` matching old_tag OR new_tag (lowercase).
/// Limit padrão 50, cap 1..500. Escopado por `user_id` igual ao rename.
async fn list_archive_tag_rename_history(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Query(q):     Query<ArchiveTagRenameHistoryQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let tag_filter = q.tag.map(|t| t.trim().to_lowercase());

    let entries: Vec<ArchiveTagRenameHistoryEntry> = sqlx::query_as(
        "SELECT id, old_tag, new_tag, renamed_count, renamed_by, renamed_at \
           FROM compliance_archive_tag_rename_history \
          WHERE tenant_id = $1 AND user_id = $2 \
            AND ($3::timestamptz IS NULL OR renamed_at >= $3) \
            AND ($4::timestamptz IS NULL OR renamed_at <  $4) \
            AND ($5::text IS NULL OR old_tag = $5 OR new_tag = $5) \
          ORDER BY renamed_at DESC \
          LIMIT $6",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(q.since)
    .bind(q.before)
    .bind(tag_filter)
    .bind(limit)
    .fetch_all(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({ "limit": limit, "entries": entries })))
}

/// POST /api/v1/compliance/archive/tags/rename-history/:id/undo — desfaz rename
/// pelo id da history (sprint #476). Paralelo do drive undo (#472). Lê entry,
/// aplica rename reverso (new→old) escopado por user_id, e grava NOVO history
/// row (com tags invertidas) tudo numa única tx via `begin_tenant_tx`. 404 se
/// id não existir no tenant. Idempotente em archives (reverted: 0 se nenhum
/// entry está mais com new_tag, mas history grava entrada do undo). Retorna
/// `{undone_id, reverted, old_tag, new_tag, history_id}` — habilita "undo do
/// undo" mantendo audit trail completo.
async fn undo_archive_tag_rename(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let entry: Option<(String, String)> = sqlx::query_as(
        "SELECT old_tag, new_tag FROM compliance_archive_tag_rename_history \
          WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let (orig_old, orig_new) = match entry {
        Some(t) => t,
        None => return Err((StatusCode::NOT_FOUND, Json(json!({"error": "history entry not found"})))),
    };

    // Reverse: undo do rename (old→new) é (new→old).
    let from = orig_new;
    let to   = orig_old;

    // Pré-DELETE de conflitos: archives que já têm `to` e também `from`.
    let _ = sqlx::query(
        "DELETE FROM compliance_archive_tags \
         WHERE tenant_id = $1 AND tag = $2 \
           AND archive_id IN ( \
               SELECT t.archive_id FROM compliance_archive_tags t \
               JOIN compliance_archive a ON a.id = t.archive_id AND a.tenant_id = t.tenant_id \
               WHERE t.tenant_id = $1 AND t.tag = $3 AND a.user_id = $4 \
           )",
    )
    .bind(ctx.tenant_id)
    .bind(&to)
    .bind(&from)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let r = sqlx::query(
        "UPDATE compliance_archive_tags SET tag = $2 \
         WHERE tenant_id = $1 AND tag = $3 \
           AND archive_id IN ( \
               SELECT id FROM compliance_archive \
               WHERE tenant_id = $1 AND user_id = $4 \
           )",
    )
    .bind(ctx.tenant_id)
    .bind(&to)
    .bind(&from)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let reverted = r.rows_affected();

    let new_history_id: (Uuid,) = sqlx::query_as(
        "INSERT INTO compliance_archive_tag_rename_history \
            (tenant_id, user_id, old_tag, new_tag, renamed_count, renamed_by) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&from)
    .bind(&to)
    .bind(reverted as i64)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({
        "undone_id":  id,
        "reverted":   reverted,
        "old_tag":    from,
        "new_tag":    to,
        "history_id": new_history_id.0,
    })))
}

#[derive(Debug, Deserialize)]
struct MergeArchiveTagBody {
    src: String,
    dst: String,
}

/// POST /api/v1/compliance/archive/tags/merge — funde duas archive tags do user
/// (sprint #483, paralelo de drive merge #433/#477). Body `{src, dst}`. Para
/// archives que têm AMBAS as tags, apaga `src` (preserva `dst`); para archives
/// que têm SÓ `src`, atualiza pra `dst`. Captura dois arrays de archive_ids
/// pra habilitar undo assimétrico (mesmo modelo do drive #477):
///   - `merged_archive_ids`: tinham só src → UPDATE pra dst → undo precisa
///     INSERT src + DELETE dst
///   - `dropped_archive_ids`: tinham ambas → DELETE src → undo precisa só
///     INSERT src de volta (dst preservada)
/// Atomicidade via begin_tenant_tx; INSERT em compliance_archive_tag_merge_history
/// com merged_count = renamed.rows_affected() (só os UPDATEs, não os DELETEs).
/// Escopado por user_id. Retorna `{src, dst, merged, dropped, history_id}`.
async fn merge_archive_tags(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Json(body):   Json<MergeArchiveTagBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let src = body.src.trim().to_lowercase();
    let dst = body.dst.trim().to_lowercase();
    if src.is_empty() || src.chars().count() > 64 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "src must be 1-64 characters"}))));
    }
    if dst.is_empty() || dst.chars().count() > 64 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "dst must be 1-64 characters"}))));
    }
    if src == dst {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "src and dst must differ"}))));
    }

    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    // archives que têm AMBAS as tags (src e dst) — vão sofrer DELETE de src
    let dropped_rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT t1.archive_id \
           FROM compliance_archive_tags t1 \
           JOIN compliance_archive_tags t2 \
             ON t2.archive_id = t1.archive_id AND t2.tenant_id = t1.tenant_id \
           JOIN compliance_archive a \
             ON a.id = t1.archive_id AND a.tenant_id = t1.tenant_id \
          WHERE t1.tenant_id = $1 AND t1.tag = $2 AND t2.tag = $3 AND a.user_id = $4",
    )
    .bind(ctx.tenant_id)
    .bind(&src)
    .bind(&dst)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
    let dropped_ids: Vec<Uuid> = dropped_rows.into_iter().map(|(id,)| id).collect();

    // Pré-DELETE de src nos archives que já tinham dst
    let _ = sqlx::query(
        "DELETE FROM compliance_archive_tags \
         WHERE tenant_id = $1 AND tag = $2 \
           AND archive_id IN ( \
               SELECT t.archive_id FROM compliance_archive_tags t \
               JOIN compliance_archive a ON a.id = t.archive_id AND a.tenant_id = t.tenant_id \
               WHERE t.tenant_id = $1 AND t.tag = $3 AND a.user_id = $4 \
           )",
    )
    .bind(ctx.tenant_id)
    .bind(&src)
    .bind(&dst)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    // archives que ainda têm src (só src; já apagamos os que tinham ambos) — vão sofrer UPDATE
    let merged_rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT t.archive_id FROM compliance_archive_tags t \
           JOIN compliance_archive a ON a.id = t.archive_id AND a.tenant_id = t.tenant_id \
          WHERE t.tenant_id = $1 AND t.tag = $2 AND a.user_id = $3",
    )
    .bind(ctx.tenant_id)
    .bind(&src)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
    let merged_ids: Vec<Uuid> = merged_rows.into_iter().map(|(id,)| id).collect();

    let r = sqlx::query(
        "UPDATE compliance_archive_tags SET tag = $2 \
         WHERE tenant_id = $1 AND tag = $3 \
           AND archive_id IN ( \
               SELECT id FROM compliance_archive \
               WHERE tenant_id = $1 AND user_id = $4 \
           )",
    )
    .bind(ctx.tenant_id)
    .bind(&dst)
    .bind(&src)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
    let merged = r.rows_affected();

    let history_id: (Uuid,) = sqlx::query_as(
        "INSERT INTO compliance_archive_tag_merge_history \
            (tenant_id, user_id, src_tag, dst_tag, merged_count, merged_archive_ids, dropped_archive_ids, merged_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING id",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&src)
    .bind(&dst)
    .bind(merged as i64)
    .bind(&merged_ids)
    .bind(&dropped_ids)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({
        "src":        src,
        "dst":        dst,
        "merged":     merged,
        "dropped":    dropped_ids.len(),
        "history_id": history_id.0,
    })))
}

#[derive(Debug, Deserialize)]
struct ArchiveTagMergeHistoryQuery {
    limit:  Option<i64>,
    since:  Option<time::OffsetDateTime>,
    before: Option<time::OffsetDateTime>,
    tag:    Option<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ArchiveTagMergeHistoryEntry {
    id:                   Uuid,
    src_tag:              String,
    dst_tag:              String,
    merged_count:         i64,
    merged_archive_ids:   Vec<Uuid>,
    dropped_archive_ids:  Vec<Uuid>,
    merged_by:            Uuid,
    #[serde(with = "time::serde::rfc3339")]
    merged_at:            time::OffsetDateTime,
}

/// GET /api/v1/compliance/archive/tags/merge-history?limit=&since=&before=&tag=
/// Audit trail de merges de tag passados pelo user (sprint #483). `tag` filtra
/// matching src_tag OR dst_tag (lowercase). Limit padrão 50, cap 1..500.
/// Escopado por user_id.
async fn list_archive_tag_merge_history(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Query(q):     Query<ArchiveTagMergeHistoryQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let tag_filter = q.tag.map(|t| t.trim().to_lowercase());

    let entries: Vec<ArchiveTagMergeHistoryEntry> = sqlx::query_as(
        "SELECT id, src_tag, dst_tag, merged_count, merged_archive_ids, dropped_archive_ids, merged_by, merged_at \
           FROM compliance_archive_tag_merge_history \
          WHERE tenant_id = $1 AND user_id = $2 \
            AND ($3::timestamptz IS NULL OR merged_at >= $3) \
            AND ($4::timestamptz IS NULL OR merged_at <  $4) \
            AND ($5::text IS NULL OR src_tag = $5 OR dst_tag = $5) \
          ORDER BY merged_at DESC \
          LIMIT $6",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(q.since)
    .bind(q.before)
    .bind(tag_filter)
    .bind(limit)
    .fetch_all(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({ "limit": limit, "entries": entries })))
}

/// POST /api/v1/compliance/archive/tags/merge-history/:id/undo — reverte um merge
/// específico (sprint #483, paralelo do drive #477). Lê arrays do history,
/// re-INSERE src nos `merged_archive_ids` (que tinham só src e foram UPDATEd)
/// e nos `dropped_archive_ids` (que tinham ambas e perderam src) — todos com
/// ON CONFLICT DO NOTHING (idempotente). DELETE dst dos `merged_archive_ids`
/// (que NÃO tinham dst originalmente). Atomicidade via begin_tenant_tx.
/// Grava nova history row com src/dst invertidos pra completar audit.
async fn undo_archive_tag_merge(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let entry: Option<(String, String, Vec<Uuid>, Vec<Uuid>)> = sqlx::query_as(
        "SELECT src_tag, dst_tag, merged_archive_ids, dropped_archive_ids \
           FROM compliance_archive_tag_merge_history \
          WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let (src, dst, merged_ids, dropped_ids) = match entry {
        Some(t) => t,
        None => return Err((StatusCode::NOT_FOUND, Json(json!({"error": "merge-history entry not found"})))),
    };

    let mut all_targets: Vec<Uuid> = Vec::with_capacity(merged_ids.len() + dropped_ids.len());
    all_targets.extend_from_slice(&merged_ids);
    all_targets.extend_from_slice(&dropped_ids);
    all_targets.sort();
    all_targets.dedup();

    // Re-insert src em todos os archives afetados (idempotente via ON CONFLICT)
    let inserted = sqlx::query(
        "INSERT INTO compliance_archive_tags (tenant_id, archive_id, tag, created_by) \
         SELECT $1, archive_id, $2, $3 FROM UNNEST($4::uuid[]) AS t(archive_id) \
         ON CONFLICT DO NOTHING",
    )
    .bind(ctx.tenant_id)
    .bind(&src)
    .bind(ctx.user_id)
    .bind(&all_targets)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
    .rows_affected();

    // DELETE dst dos archives que ANTES não tinham dst (merged_ids only) —
    // dropped_ids tinham dst desde sempre, preservar
    let deleted = sqlx::query(
        "DELETE FROM compliance_archive_tags \
          WHERE tenant_id = $1 AND tag = $2 \
            AND archive_id = ANY($3::uuid[])",
    )
    .bind(ctx.tenant_id)
    .bind(&dst)
    .bind(&merged_ids)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
    .rows_affected();

    let new_history_id: (Uuid,) = sqlx::query_as(
        "INSERT INTO compliance_archive_tag_merge_history \
            (tenant_id, user_id, src_tag, dst_tag, merged_count, merged_archive_ids, dropped_archive_ids, merged_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING id",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&dst)
    .bind(&src)
    .bind(deleted as i64)
    .bind(&merged_ids)
    .bind(&dropped_ids)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({
        "undone_id":      id,
        "src":            src,
        "dst":            dst,
        "src_reinserted": inserted,
        "dst_deleted":    deleted,
        "history_id":     new_history_id.0,
    })))
}

/// GET /api/v1/compliance/archive/export — download all matching archive entries as a ZIP.
///
/// Accepts the same `since`, `before`, `subject`, `from_addr`, `to_addr`, `size_min`, `size_max`
/// filters as the list endpoint. Returns a ZIP containing:
///   - `manifest.json` — JSON array of all entry metadata
///   - `messages/<id>.eml` — raw message bytes for each entry (best-effort; skipped on I/O error)
async fn export_archive(
    State(st):     State<AppState>,
    AuthCtx(ctx):  AuthCtx,
    Query(params): Query<ArchiveListParams>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    // Build the same filter clauses as list_archive but without pagination.
    let since_filter = params.since.as_deref()
        .map(|d| format!("AND archived_at >= '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let before_date_filter = params.before.as_deref()
        .map(|d| format!("AND archived_at < '{}'::timestamptz", d.replace('\'', "''")))
        .unwrap_or_default();
    let subject_filter = params.subject.as_deref()
        .map(|s| format!("AND subject ILIKE '%{}%'", s.replace('\'', "''")))
        .unwrap_or_default();
    let from_addr_filter = params.from_addr.as_deref()
        .map(|s| format!("AND from_addr ILIKE '%{}%'", s.replace('\'', "''")))
        .unwrap_or_default();
    let to_addr_filter = params.to_addr.as_deref()
        .map(|s| format!("AND to_addrs::text ILIKE '%{}%'", s.replace('\'', "''")))
        .unwrap_or_default();
    let size_min_filter = params.size_min
        .map(|v| format!("AND size_bytes >= {v}"))
        .unwrap_or_default();
    let size_max_filter = params.size_max
        .map(|v| format!("AND size_bytes <= {v}"))
        .unwrap_or_default();

    let sql = format!(
        "SELECT id, tenant_id, user_id, original_id, body_path, from_addr, \
                to_addrs, subject, archived_at, size_bytes \
         FROM compliance_archive \
         WHERE tenant_id = $1 AND user_id = $2 \
         {since_filter} {before_date_filter} {subject_filter} {from_addr_filter} \
         {to_addr_filter} {size_min_filter} {size_max_filter} \
         ORDER BY archived_at ASC"
    );

    let rows: Vec<ArchiveEntry> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_all(&st.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    // Build ZIP in memory.
    let buf = std::io::Cursor::new(Vec::new());
    let mut zw = zip::ZipWriter::new(buf);
    // When a password is provided, apply AES-256 encryption to every file entry.
    let pw_ref: Option<&str> = params.password.as_deref();
    macro_rules! file_options {
        () => {{
            let o = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            if let Some(pw) = pw_ref { o.with_aes_encryption(zip::AesMode::Aes256, pw) } else { o }
        }};
    }

    // manifest.json
    let manifest = serde_json::to_vec(&rows)
        .unwrap_or_else(|_| b"[]".to_vec());
    zw.start_file("manifest.json", file_options!())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
    std::io::Write::write_all(&mut zw, &manifest)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    // One .eml file per entry — best-effort, skip unreadable files.
    for entry in &rows {
        if let Ok(bytes) = tokio::fs::read(&entry.body_path).await {
            let name = format!("messages/{}.eml", entry.id);
            if zw.start_file(&name, file_options!()).is_ok() {
                let _ = std::io::Write::write_all(&mut zw, &bytes);
            }
        }
    }

    let buf = zw.finish()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
    let zip_bytes = buf.into_inner();

    // HMAC-SHA256 signature over ZIP bytes, keyed by COMPLIANCE__EXPORT_SECRET.
    // Emitted as hex in X-Export-Signature; omitted when the env var is absent.
    let signature = env::var("COMPLIANCE__EXPORT_SECRET").ok().map(|secret| {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(&zip_bytes);
        hex::encode(mac.finalize().into_bytes())
    });

    let mut resp = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE,        "application/zip".to_string()),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"compliance-export.zip\"".to_string()),
            (header::CONTENT_LENGTH,      zip_bytes.len().to_string()),
        ],
        zip_bytes,
    ).into_response();
    if let Some(sig) = signature {
        resp.headers_mut().insert(
            header::HeaderName::from_static("x-export-signature"),
            HeaderValue::from_str(&sig).unwrap(),
        );
    }
    Ok(resp)
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
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = time::OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if entry.archived_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
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

// ─── Archive entry tags (sprint #460) ────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ArchiveTagBody { tag: String }

/// Garante que o entry exista no tenant/user antes de mexer em tags. Reusa o
/// mesmo filtro de get_archive_entry/delete_archive_entry pra consistência.
async fn assert_archive_entry_exists(
    db: &expresso_core::DbPool,
    id: Uuid,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM compliance_archive \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id).bind(tenant_id).bind(user_id)
    .fetch_optional(db).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
    if exists.is_none() {
        return Err((StatusCode::NOT_FOUND, Json(json!({"error": "archive entry not found"}))));
    }
    Ok(())
}

/// POST /api/v1/compliance/archive/:id/tags — adiciona uma tag a um entry do
/// archive (sprint #460). Tag normalizada lowercase + trim, validação 1-64
/// chars (mesmo schema do drive_file_tags). UNIQUE (archive_id, tenant_id, tag)
/// torna a operação idempotente — re-POST do mesmo par não erra. Útil pra
/// rotular evidências em e-discovery (case-IDs, hold-tags, classificação
/// regulatória) sem mexer no schema do archive em si.
async fn add_archive_tag(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<ArchiveTagBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let tag = body.tag.trim().to_lowercase();
    if tag.is_empty() || tag.chars().count() > 64 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "tag must be 1-64 characters"}))));
    }
    assert_archive_entry_exists(&st.db, id, ctx.tenant_id, ctx.user_id).await?;

    let row: (Uuid, Uuid, Uuid, String, Uuid, time::OffsetDateTime) = sqlx::query_as(
        "INSERT INTO compliance_archive_tags (archive_id, tenant_id, tag, created_by) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (archive_id, tenant_id, tag) DO UPDATE \
             SET created_at = compliance_archive_tags.created_at \
         RETURNING id, archive_id, tenant_id, tag, created_by, created_at",
    )
    .bind(id).bind(ctx.tenant_id).bind(&tag).bind(ctx.user_id)
    .fetch_one(&st.db).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok((StatusCode::CREATED, Json(json!({
        "id":         row.0,
        "archive_id": row.1,
        "tenant_id":  row.2,
        "tag":        row.3,
        "created_by": row.4,
        "created_at": row.5.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
    }))))
}

/// GET /api/v1/compliance/archive/:id/tags — lista tags de um entry do archive
/// (sprint #460). Retorna `{tags: ["...", ...]}` ordenado alfabeticamente. 404
/// se o entry não existe ou não pertence ao user.
async fn list_archive_tags(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    assert_archive_entry_exists(&st.db, id, ctx.tenant_id, ctx.user_id).await?;

    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT tag FROM compliance_archive_tags \
         WHERE archive_id = $1 AND tenant_id = $2 \
         ORDER BY tag ASC",
    )
    .bind(id).bind(ctx.tenant_id)
    .fetch_all(&st.db).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let tags: Vec<String> = rows.into_iter().map(|(t,)| t).collect();
    Ok(Json(json!({ "archive_id": id, "tags": tags })))
}

/// DELETE /api/v1/compliance/archive/:id/tags/:tag — remove uma tag de um entry
/// (sprint #460). 404 se o par (archive_id, tag) não existir. Tag normalizada
/// lowercase pra match com add_archive_tag.
async fn remove_archive_tag(
    State(st):       State<AppState>,
    AuthCtx(ctx):    AuthCtx,
    Path((id, tag)): Path<(Uuid, String)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let tag = tag.trim().to_lowercase();
    let r = sqlx::query(
        "DELETE FROM compliance_archive_tags \
         WHERE archive_id = $1 AND tenant_id = $2 AND tag = $3",
    )
    .bind(id).bind(ctx.tenant_id).bind(&tag)
    .execute(&st.db).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    if r.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(json!({"error": "tag not found on entry"}))));
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

// ─── Tenant-wide retention ────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct TenantRetention {
    tenant_id:   Uuid,
    retain_days: i32,
    #[serde(with = "time::serde::rfc3339")]
    updated_at:  time::OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct SetTenantRetentionRequest {
    retain_days: i32,
}

/// GET /api/v1/compliance/retention — current tenant-wide default retention days.
///
/// Returns 200 with the current setting, or a default of 365 days when not set.
async fn get_tenant_retention(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    req_headers:  axum::http::HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let row: Option<TenantRetention> = sqlx::query_as(
        "SELECT tenant_id, retain_days, updated_at \
         FROM compliance_tenant_retention WHERE tenant_id = $1",
    )
    .bind(ctx.tenant_id)
    .fetch_optional(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let (retention, updated_at) = match row {
        Some(r) => {
            let ts = r.updated_at;
            (r, Some(ts))
        }
        None => (
            TenantRetention {
                tenant_id:   ctx.tenant_id,
                retain_days: 365,
                updated_at:  time::OffsetDateTime::UNIX_EPOCH,
            },
            None,
        ),
    };

    if let Some(ts) = updated_at {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = time::OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
        let mut resp = Json(&retention).into_response();
        resp.headers_mut().insert(header::LAST_MODIFIED, axum::http::HeaderValue::from_str(&lm).unwrap());
        return Ok(resp);
    }

    Ok(Json(retention).into_response())
}

/// PUT /api/v1/compliance/retention — set or update tenant-wide default retention days.
async fn put_tenant_retention(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Json(body):   Json<SetTenantRetentionRequest>,
) -> Result<Json<TenantRetention>, (StatusCode, Json<serde_json::Value>)> {
    if body.retain_days <= 0 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "retain_days must be > 0"}))));
    }
    if body.retain_days > 36500 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "retain_days must be <= 36500 (100 years)"}))));
    }
    let row: TenantRetention = sqlx::query_as(
        "INSERT INTO compliance_tenant_retention (tenant_id, retain_days, updated_at) \
         VALUES ($1, $2, now()) \
         ON CONFLICT (tenant_id) DO UPDATE SET \
            retain_days = EXCLUDED.retain_days, \
            updated_at  = now() \
         RETURNING tenant_id, retain_days, updated_at",
    )
    .bind(ctx.tenant_id)
    .bind(body.retain_days)
    .fetch_one(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tracing::info!(target: "audit",
        event = "compliance.retention.set",
        tenant_id = %ctx.tenant_id, retain_days = body.retain_days);
    Ok(Json(row))
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
        .route("/api/v1/compliance/archive/count",     get(count_archive))
        .route("/api/v1/compliance/archive/histogram", get(histogram_archive))
        .route("/api/v1/compliance/archive/top-senders", get(top_senders_archive))
        .route("/api/v1/compliance/archive/top-recipients", get(top_recipients_archive))
        .route("/api/v1/compliance/archive/top-subjects", get(top_subjects_archive))
        .route("/api/v1/compliance/archive/top-domains", get(top_domains_archive))
        .route("/api/v1/compliance/archive/size-histogram", get(size_histogram_archive))
        .route("/api/v1/compliance/archive/top-tags",  get(top_tags_archive))
        .route("/api/v1/compliance/archive/tags/intersect", get(archive_entries_intersect))
        .route("/api/v1/compliance/archive/tags/union", get(archive_entries_union))
        .route("/api/v1/compliance/archive/tags/rename-history", get(list_archive_tag_rename_history))
        .route("/api/v1/compliance/archive/tags/rename-history/:id/undo", post(undo_archive_tag_rename))
        .route("/api/v1/compliance/archive/tags/merge", post(merge_archive_tags))
        .route("/api/v1/compliance/archive/tags/merge-history", get(list_archive_tag_merge_history))
        .route("/api/v1/compliance/archive/tags/merge-history/:id/undo", post(undo_archive_tag_merge))
        .route("/api/v1/compliance/archive/tags/:tag", get(archive_entries_by_tag).patch(rename_archive_tag))
        .route("/api/v1/compliance/archive/export",    get(export_archive))
        .route("/api/v1/compliance/archive/:id",       get(get_archive_entry).delete(delete_archive_entry))
        .route("/api/v1/compliance/archive/:id/tags",  get(list_archive_tags).post(add_archive_tag))
        .route("/api/v1/compliance/archive/:id/tags/:tag", delete(remove_archive_tag))
        .route("/api/v1/compliance/retention",         get(get_tenant_retention).put(put_tenant_retention))
        .merge(expresso_observability::metrics_router())
        .layer(middleware::from_fn_with_state(state.clone(), inject_validator))
        .with_state(state);

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(service = SERVICE, %addr, "listening");
    axum::serve(listener, app).await?;

    Ok(())
}

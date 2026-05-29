//! expresso-flows — user-defined workflow rules for incoming mail.
//!
//! Rules are evaluated after delivery (called by expresso-mail via
//! POST /internal/process). Each rule has conditions and actions:
//!
//! Conditions: [{field: "from"|"subject"|"folder"|"to", op: "contains"|"equals"|"starts_with", value: "..."}]
//! Actions:    [{type: "move_to_folder", params: {folder: "..."}}]
//!             [{type: "add_flag",       params: {flag: "..."}}]
//!             [{type: "webhook",        params: {url: "..."}}]
//!
//! Rules are matched in ascending priority order. Multiple rules can match
//! (non-exclusive). The caller receives the list of actions to execute.
//!
//! `condition_mode`: "and" (default) — all conditions must match;
//!                   "or"            — any condition suffices.
//!
//! CRUD: GET/POST/PATCH/DELETE /api/v1/flows/rules        (JWT auth; ?enabled=true/false filter)
//! GET  /api/v1/flows/rules/:id                           (JWT auth; returns ETag + Last-Modified)
//! Reorder: PATCH  /api/v1/flows/rules/reorder            (JWT auth) — bulk priority update
//! Bulk delete: DELETE /api/v1/flows/rules/bulk           (JWT auth) — delete multiple rules
//! Trigger: POST /internal/process                        (internal, no auth)
//!
//! Port: :8005

use std::{env, net::SocketAddr, sync::Arc};

use axum::async_trait;
use axum::{
    extract::{FromRequestParts, Path, Query, Request, State},
    http::{header, request::Parts, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use expresso_auth_client::{AuthContext, AuthRejection, Authenticated, OidcConfig, OidcValidator};
use expresso_core::{begin_tenant_tx, create_db_pool, init_tracing, run_migrations, AppConfig};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::info;
use uuid::Uuid;

const SERVICE: &str = "expresso-flows";
const DEFAULT_PORT: u16 = 8005;

// ─── App state ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    db: expresso_core::DbPool,
    validator: Option<Arc<OidcValidator>>,
}

// ─── DTOs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct FlowRule {
    pub id: Uuid,
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub priority: i32,
    pub conditions: serde_json::Value,
    pub condition_mode: String,
    pub actions: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct CreateRuleRequest {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub conditions: serde_json::Value,
    /// "and" (default) or "or"
    pub condition_mode: Option<String>,
    pub actions: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct UpdateRuleRequest {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub conditions: Option<serde_json::Value>,
    pub condition_mode: Option<String>,
    pub actions: Option<serde_json::Value>,
}

/// Query params for GET /api/v1/flows/rules
#[derive(Debug, Deserialize)]
struct ListRulesParams {
    /// Filter by enabled status. Omit to return all rules.
    pub enabled: Option<bool>,
    /// ILIKE filter on rule name. E.g. `?name=invoice` matches "Invoice alerts".
    pub name: Option<String>,
    /// Return only rules with priority >= this value.
    pub priority_min: Option<i32>,
    /// Return only rules with priority <= this value.
    pub priority_max: Option<i32>,
    /// Filter by condition_mode: "and" or "or". Omit to return all.
    pub condition_mode: Option<String>,
    /// Return only rules that contain at least one action of this type.
    /// E.g. `?action_type=move_to_folder`, `?action_type=webhook`.
    pub action_type: Option<String>,
    /// Maximum number of rules to return (default 100, max 500).
    pub limit: Option<i64>,
    /// Zero-indexed page for offset pagination (default 0).
    pub page: Option<i64>,
}

/// One entry in a bulk reorder request.
#[derive(Debug, Deserialize)]
struct PriorityEntry {
    pub id: Uuid,
    pub priority: i32,
}

#[derive(Debug, Serialize)]
struct ReorderResult {
    pub updated: usize,
}

#[derive(Debug, Deserialize)]
struct BulkDeleteRulesRequest {
    pub ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
struct BulkDeleteResult {
    pub deleted: u64,
}

/// Payload from expresso-mail: metadata about the delivered message.
#[derive(Debug, Deserialize)]
struct ProcessRequest {
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub message_id: Uuid,
    pub folder: String,
    pub from_addr: Option<String>,
    pub to_addrs: Option<Vec<String>>,
    pub subject: Option<String>,
    pub has_attachments: Option<bool>,
    pub size_bytes: Option<i32>,
}

#[derive(Debug, Serialize)]
struct ProcessResponse {
    pub matched_rules: usize,
    pub actions: Vec<serde_json::Value>,
}

// ─── Auth helpers ─────────────────────────────────────────────────────────────

struct AuthCtx(AuthContext);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for AuthCtx {
    type Rejection = AuthRejection;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Authenticated(ctx) = Authenticated::from_request_parts(parts, state).await?;
        Ok(AuthCtx(ctx))
    }
}

async fn inject_validator(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    if let Some(v) = &st.validator {
        req.extensions_mut().insert(v.clone());
    }
    next.run(req).await
}

// ─── CRUD handlers ────────────────────────────────────────────────────────────

async fn list_rules(
    State(st): State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Query(params): Query<ListRulesParams>,
    req_headers: HeaderMap,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.limit.unwrap_or(100).min(500);
    let offset = params.page.unwrap_or(0) * limit;

    let enabled_filter = match params.enabled {
        Some(true) => "AND enabled = TRUE",
        Some(false) => "AND enabled = FALSE",
        None => "",
    };
    let name_filter = params
        .name
        .map(|n| {
            let esc = n
                .replace('\'', "''")
                .replace('%', "\\%")
                .replace('_', "\\_");
            format!("AND name ILIKE '%{esc}%'")
        })
        .unwrap_or_default();
    let priority_min_filter = params
        .priority_min
        .map(|v| format!("AND priority >= {v}"))
        .unwrap_or_default();
    let priority_max_filter = params
        .priority_max
        .map(|v| format!("AND priority <= {v}"))
        .unwrap_or_default();
    let condition_mode_filter = params
        .condition_mode
        .map(|m| {
            let esc = m.replace('\'', "''");
            format!("AND condition_mode = '{esc}'")
        })
        .unwrap_or_default();
    let action_type_filter = params.action_type.map(|t| {
        let esc = t.replace('\'', "''");
        format!("AND EXISTS (SELECT 1 FROM jsonb_array_elements(actions) a WHERE a->>'type' = '{esc}')")
    }).unwrap_or_default();

    let sql = format!(
        "SELECT id, user_id, tenant_id, name, enabled, priority, conditions, condition_mode, actions \
         FROM flow_rules \
         WHERE tenant_id = $1 AND user_id = $2 {enabled_filter} {name_filter} \
         {priority_min_filter} {priority_max_filter} {condition_mode_filter} {action_type_filter} \
         ORDER BY priority ASC, created_at ASC \
         LIMIT {limit} OFFSET {offset}"
    );

    let max_updated: Option<time::OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(updated_at) FROM flow_rules WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(&st.db)
    .await
    .unwrap_or(None);

    if let Some(ts) = max_updated {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = time::OffsetDateTime::parse(
                    ims_str,
                    &time::format_description::well_known::Rfc2822,
                ) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

    let rows: Vec<FlowRule> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_all(&st.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    let count_sql = format!(
        "SELECT COUNT(*) FROM flow_rules \
         WHERE tenant_id = $1 AND user_id = $2 {enabled_filter} {name_filter} \
         {priority_min_filter} {priority_max_filter} {condition_mode_filter} {action_type_filter}"
    );
    let total: i64 = sqlx::query_scalar(&count_sql)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_one(&st.db)
        .await
        .unwrap_or(0);

    let mut resp = (
        StatusCode::OK,
        [(
            header::HeaderName::from_static("x-total-count"),
            total.to_string(),
        )],
        Json(rows),
    )
        .into_response();
    if let Some(ts) = max_updated {
        let lm = ts
            .format(&time::format_description::well_known::Rfc2822)
            .unwrap_or_default();
        resp.headers_mut()
            .insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

async fn create_rule(
    State(st): State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Json(req): Json<CreateRuleRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    let rule: FlowRule = sqlx::query_as(
        "INSERT INTO flow_rules (user_id, tenant_id, name, enabled, priority, conditions, condition_mode, actions) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING id, user_id, tenant_id, name, enabled, priority, conditions, condition_mode, actions",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(req.name.unwrap_or_default())
    .bind(req.enabled.unwrap_or(true))
    .bind(req.priority.unwrap_or(10))
    .bind(&req.conditions)
    .bind(req.condition_mode.unwrap_or_else(|| "and".into()))
    .bind(&req.actions)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tx.commit().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    let location = format!("/api/v1/flows/rules/{}", rule.id);
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(rule),
    ))
}

async fn update_rule(
    State(st): State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateRuleRequest>,
) -> Result<Json<FlowRule>, (StatusCode, Json<serde_json::Value>)> {
    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    let rule: Option<FlowRule> = sqlx::query_as(
        "UPDATE flow_rules \
         SET name           = COALESCE($3, name), \
             enabled        = COALESCE($4, enabled), \
             priority       = COALESCE($5, priority), \
             conditions     = COALESCE($6, conditions), \
             condition_mode = COALESCE($7, condition_mode), \
             actions        = COALESCE($8, actions), \
             updated_at     = NOW() \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $9 \
         RETURNING id, user_id, tenant_id, name, enabled, priority, conditions, condition_mode, actions",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(req.name)
    .bind(req.enabled)
    .bind(req.priority)
    .bind(req.conditions)
    .bind(req.condition_mode)
    .bind(req.actions)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tx.commit().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    match rule {
        Some(r) => Ok(Json(r)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "rule not found"})),
        )),
    }
}

/// GET /api/v1/flows/rules/:id — ETag + Last-Modified; responds 304 if If-None-Match or If-Modified-Since matches.
async fn get_rule(
    State(st): State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id): Path<Uuid>,
    req_headers: HeaderMap,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    #[derive(sqlx::FromRow)]
    struct RuleRow {
        id: Uuid,
        user_id: Uuid,
        tenant_id: Uuid,
        name: String,
        enabled: bool,
        priority: i32,
        conditions: serde_json::Value,
        condition_mode: String,
        actions: serde_json::Value,
        updated_at: time::OffsetDateTime,
    }

    let row: Option<RuleRow> = sqlx::query_as(
        "SELECT id, user_id, tenant_id, name, enabled, priority, conditions, condition_mode, \
                actions, updated_at \
         FROM flow_rules \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&st.db)
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    let r = row.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "rule not found"})),
        )
    })?;

    let etag = format!("\"{}-{}\"", r.updated_at.unix_timestamp(), r.id);
    let last_modified = r
        .updated_at
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap_or_default();

    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) =
                time::OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822)
            {
                if r.updated_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }

    let rule = FlowRule {
        id: r.id,
        user_id: r.user_id,
        tenant_id: r.tenant_id,
        name: r.name,
        enabled: r.enabled,
        priority: r.priority,
        conditions: r.conditions,
        condition_mode: r.condition_mode,
        actions: r.actions,
    };

    Ok((
        StatusCode::OK,
        [(header::ETAG, etag), (header::LAST_MODIFIED, last_modified)],
        Json(rule),
    )
        .into_response())
}

async fn delete_rule(
    State(st): State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    sqlx::query("DELETE FROM flow_rules WHERE id = $1 AND tenant_id = $2 AND user_id = $3")
        .bind(id)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    tx.commit().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    Ok(StatusCode::NO_CONTENT)
}

/// PATCH /api/v1/flows/rules/:id/toggle — flip enabled ↔ disabled atomically.
/// Returns the updated rule. 404 if not found.
async fn toggle_rule(
    State(st): State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<FlowRule>, (StatusCode, Json<serde_json::Value>)> {
    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    let row: Option<FlowRule> = sqlx::query_as(
        "UPDATE flow_rules SET enabled = NOT enabled \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3 \
         RETURNING id, user_id, tenant_id, name, enabled, priority, conditions, condition_mode, actions",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tx.commit().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    match row {
        Some(r) => Ok(Json(r)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "rule not found"})),
        )),
    }
}

// ─── Bulk delete handler ──────────────────────────────────────────────────────

/// DELETE /api/v1/flows/rules/bulk — delete multiple rules in one request.
/// Body: `{"ids": ["<uuid>", …]}`
/// Only deletes rules owned by the authenticated user+tenant. Returns count deleted.
async fn bulk_delete_rules(
    State(st): State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Json(req): Json<BulkDeleteRulesRequest>,
) -> Result<Json<BulkDeleteResult>, (StatusCode, Json<serde_json::Value>)> {
    if req.ids.is_empty() {
        return Ok(Json(BulkDeleteResult { deleted: 0 }));
    }

    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    let res = sqlx::query(
        "DELETE FROM flow_rules \
         WHERE id = ANY($1) AND tenant_id = $2 AND user_id = $3",
    )
    .bind(&req.ids)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    tx.commit().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    Ok(Json(BulkDeleteResult {
        deleted: res.rows_affected(),
    }))
}

// ─── Reorder handler ──────────────────────────────────────────────────────────

/// PATCH /api/v1/flows/rules/reorder — bulk priority assignment.
/// Body: `[{"id": "<uuid>", "priority": N}, …]`
/// Only updates rules owned by the authenticated user+tenant.
async fn reorder_rules(
    State(st): State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Json(entries): Json<Vec<PriorityEntry>>,
) -> Result<Json<ReorderResult>, (StatusCode, Json<serde_json::Value>)> {
    if entries.is_empty() {
        return Ok(Json(ReorderResult { updated: 0 }));
    }

    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    let mut updated = 0usize;
    for entry in &entries {
        let rows = sqlx::query(
            "UPDATE flow_rules \
             SET priority = $3, updated_at = NOW() \
             WHERE id = $1 AND tenant_id = $2 AND user_id = $4",
        )
        .bind(entry.id)
        .bind(ctx.tenant_id)
        .bind(entry.priority)
        .bind(ctx.user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
        updated += rows.rows_affected() as usize;
    }

    tx.commit().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    Ok(Json(ReorderResult { updated }))
}

// ─── Internal process handler ─────────────────────────────────────────────────

/// POST /internal/process — evaluate rules for a freshly delivered message.
/// Returns the list of actions from all matching rules (caller executes them).
async fn internal_process(
    State(st): State<AppState>,
    Json(req): Json<ProcessRequest>,
) -> Json<ProcessResponse> {
    let rules: Vec<FlowRule> = match sqlx::query_as(
        "SELECT id, user_id, tenant_id, name, enabled, priority, conditions, condition_mode, actions \
         FROM flow_rules \
         WHERE tenant_id = $1 AND user_id = $2 AND enabled = TRUE \
         ORDER BY priority ASC",
    )
    .bind(req.tenant_id)
    .bind(req.user_id)
    .fetch_all(&st.db)
    .await {
        Ok(r)  => r,
        Err(e) => {
            tracing::warn!(error = %e, "flows: DB error fetching rules");
            return Json(ProcessResponse { matched_rules: 0, actions: vec![] });
        }
    };

    let mut matched = 0usize;
    let mut actions: Vec<serde_json::Value> = vec![];

    for rule in &rules {
        if rule_matches(&rule.conditions, &rule.condition_mode, &req) {
            matched += 1;
            if let Some(arr) = rule.actions.as_array() {
                for a in arr {
                    let mut entry = a.clone();
                    entry["rule_id"] = json!(rule.id);
                    entry["message_id"] = json!(req.message_id);
                    actions.push(entry);
                }
            }
        }
    }

    Json(ProcessResponse {
        matched_rules: matched,
        actions,
    })
}

/// Evaluate all conditions for a rule.
/// `condition_mode`: "or" — any single condition suffices; "and" (default) — all must match.
fn rule_matches(
    conditions: &serde_json::Value,
    condition_mode: &str,
    req: &ProcessRequest,
) -> bool {
    let conds = match conditions.as_array() {
        Some(a) if !a.is_empty() => a,
        _ => return true, // no conditions = always match
    };

    let is_or = condition_mode.eq_ignore_ascii_case("or");

    for cond in conds {
        let field = cond.get("field").and_then(|v| v.as_str()).unwrap_or("");
        let op = cond
            .get("op")
            .and_then(|v| v.as_str())
            .unwrap_or("contains");
        let val = cond.get("value").and_then(|v| v.as_str()).unwrap_or("");

        let cond_result = eval_condition(field, op, val, req);

        if is_or {
            if cond_result {
                return true;
            }
        } else if !cond_result {
            return false;
        }
    }

    // AND: all passed; OR: none passed.
    !is_or
}

fn eval_condition(field: &str, op: &str, val: &str, req: &ProcessRequest) -> bool {
    match field {
        "from" => req.from_addr.as_deref().is_some_and(|h| str_op(h, op, val)),
        "subject" => req.subject.as_deref().is_some_and(|h| str_op(h, op, val)),
        "folder" => str_op(&req.folder, op, val),
        "to" => {
            let addrs = req.to_addrs.as_deref().unwrap_or(&[]);
            addrs.iter().any(|a| str_op(a, op, val))
        }
        "has_attachment" => {
            let want = val.eq_ignore_ascii_case("true");
            req.has_attachments.unwrap_or(false) == want
        }
        "size" => {
            let threshold = val.trim().parse::<i32>().unwrap_or(0);
            let actual = req.size_bytes.unwrap_or(0);
            match op {
                "gt" | "greater_than" => actual > threshold,
                "lt" | "less_than" => actual < threshold,
                "gte" | "greater_than_or_equal" => actual >= threshold,
                "lte" | "less_than_or_equal" => actual <= threshold,
                _ => actual == threshold,
            }
        }
        _ => false,
    }
}

fn str_op(hay: &str, op: &str, needle: &str) -> bool {
    let hay_lc = hay.to_lowercase();
    let needle_lc = needle.to_lowercase();
    match op {
        "equals" => hay_lc == needle_lc,
        "starts_with" => hay_lc.starts_with(&needle_lc),
        _ => hay_lc.contains(&needle_lc), // "contains" is default
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
    let issuer = env::var("AUTH__OIDC_ISSUER")
        .ok()
        .filter(|v| !v.is_empty())?;
    let audience = env::var("AUTH__OIDC_AUDIENCE")
        .ok()
        .filter(|v| !v.is_empty())?;
    let cfg = OidcConfig::new(issuer.clone(), audience);
    match OidcValidator::new(cfg).await {
        Ok(v) => {
            info!(issuer = %issuer, "OIDC validator ready");
            Some(Arc::new(v))
        }
        Err(e) => {
            tracing::warn!(error = %e, "OIDC init failed — no JWT auth");
            None
        }
    }
}

fn resolve_addr() -> anyhow::Result<SocketAddr> {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    format!("{host}:{port}")
        .parse::<SocketAddr>()
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

    let validator = maybe_build_validator().await;
    let state = AppState { db, validator };

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/internal/process", post(internal_process))
        .route("/api/v1/flows/rules", get(list_rules).post(create_rule))
        .route("/api/v1/flows/rules/reorder", patch(reorder_rules))
        .route("/api/v1/flows/rules/bulk", delete(bulk_delete_rules))
        .route("/api/v1/flows/rules/:id/toggle", patch(toggle_rule))
        .route(
            "/api/v1/flows/rules/:id",
            get(get_rule).patch(update_rule).delete(delete_rule),
        )
        .merge(expresso_observability::metrics_router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            inject_validator,
        ))
        .with_state(state);

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(service = SERVICE, %addr, "listening");
    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn req(
        folder: &str,
        from: Option<&str>,
        subject: Option<&str>,
        to: Option<Vec<&str>>,
        has_attachments: Option<bool>,
        size_bytes: Option<i32>,
    ) -> ProcessRequest {
        ProcessRequest {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            message_id: Uuid::new_v4(),
            folder: folder.into(),
            from_addr: from.map(Into::into),
            to_addrs: to.map(|v| v.into_iter().map(Into::into).collect()),
            subject: subject.map(Into::into),
            has_attachments,
            size_bytes,
        }
    }

    fn cond(field: &str, op: &str, value: &str) -> serde_json::Value {
        serde_json::json!([{"field": field, "op": op, "value": value}])
    }

    // ── str_op ────────────────────────────────────────────────────────────────

    #[test]
    fn str_op_contains_matches_substring() {
        assert!(str_op("Hello World", "contains", "world"));
    }

    #[test]
    fn str_op_contains_no_match() {
        assert!(!str_op("Hello", "contains", "xyz"));
    }

    #[test]
    fn str_op_equals_exact() {
        assert!(str_op("INBOX", "equals", "inbox"));
    }

    #[test]
    fn str_op_equals_case_insensitive() {
        assert!(str_op("Invoice", "equals", "invoice"));
    }

    #[test]
    fn str_op_equals_no_match() {
        assert!(!str_op("INBOX", "equals", "Sent"));
    }

    #[test]
    fn str_op_starts_with_matches() {
        assert!(str_op("Invoice 2026", "starts_with", "invoice"));
    }

    #[test]
    fn str_op_starts_with_no_match() {
        assert!(!str_op("Hello Invoice", "starts_with", "invoice"));
    }

    #[test]
    fn str_op_unknown_op_falls_back_to_contains() {
        assert!(str_op("foobar", "regex", "bar"));
    }

    // ── eval_condition ────────────────────────────────────────────────────────

    #[test]
    fn eval_condition_from_contains() {
        let r = req("INBOX", Some("boss@corp.com"), None, None, None, None);
        assert!(eval_condition("from", "contains", "corp.com", &r));
    }

    #[test]
    fn eval_condition_from_no_match() {
        let r = req("INBOX", Some("other@example.com"), None, None, None, None);
        assert!(!eval_condition("from", "contains", "corp.com", &r));
    }

    #[test]
    fn eval_condition_from_none_returns_false() {
        let r = req("INBOX", None, None, None, None, None);
        assert!(!eval_condition("from", "contains", "corp.com", &r));
    }

    #[test]
    fn eval_condition_subject_equals() {
        let r = req("INBOX", None, Some("Invoice"), None, None, None);
        assert!(eval_condition("subject", "equals", "invoice", &r));
    }

    #[test]
    fn eval_condition_folder_matches() {
        let r = req("Spam", None, None, None, None, None);
        assert!(eval_condition("folder", "equals", "spam", &r));
    }

    #[test]
    fn eval_condition_to_any_addr_matches() {
        let r = req(
            "INBOX",
            None,
            None,
            Some(vec!["a@x.com", "b@x.com"]),
            None,
            None,
        );
        assert!(eval_condition("to", "contains", "b@x.com", &r));
    }

    #[test]
    fn eval_condition_to_empty_vec_returns_false() {
        let r = req("INBOX", None, None, Some(vec![]), None, None);
        assert!(!eval_condition("to", "contains", "anyone", &r));
    }

    #[test]
    fn eval_condition_has_attachment_true() {
        let r = req("INBOX", None, None, None, Some(true), None);
        assert!(eval_condition("has_attachment", "equals", "true", &r));
    }

    #[test]
    fn eval_condition_has_attachment_false_when_none() {
        let r = req("INBOX", None, None, None, None, None);
        assert!(!eval_condition("has_attachment", "equals", "true", &r));
    }

    #[test]
    fn eval_condition_size_gt() {
        let r = req("INBOX", None, None, None, None, Some(5000));
        assert!(eval_condition("size", "gt", "1000", &r));
    }

    #[test]
    fn eval_condition_size_lt() {
        let r = req("INBOX", None, None, None, None, Some(100));
        assert!(eval_condition("size", "lt", "1000", &r));
    }

    #[test]
    fn eval_condition_size_gte_equal() {
        let r = req("INBOX", None, None, None, None, Some(1000));
        assert!(eval_condition("size", "gte", "1000", &r));
    }

    #[test]
    fn eval_condition_size_lte_equal() {
        let r = req("INBOX", None, None, None, None, Some(1000));
        assert!(eval_condition("size", "lte", "1000", &r));
    }

    #[test]
    fn eval_condition_unknown_field_returns_false() {
        let r = req("INBOX", None, None, None, None, None);
        assert!(!eval_condition("nonexistent", "equals", "val", &r));
    }

    // ── rule_matches ──────────────────────────────────────────────────────────

    #[test]
    fn rule_matches_empty_conditions_always_true() {
        let r = req("INBOX", None, None, None, None, None);
        assert!(rule_matches(&serde_json::json!([]), "and", &r));
    }

    #[test]
    fn rule_matches_null_conditions_always_true() {
        let r = req("INBOX", None, None, None, None, None);
        assert!(rule_matches(&serde_json::Value::Null, "and", &r));
    }

    #[test]
    fn rule_matches_and_all_match() {
        let r = req(
            "INBOX",
            Some("boss@corp.com"),
            Some("Invoice"),
            None,
            None,
            None,
        );
        let conds = serde_json::json!([
            {"field": "from", "op": "contains", "value": "corp.com"},
            {"field": "subject", "op": "contains", "value": "invoice"}
        ]);
        assert!(rule_matches(&conds, "and", &r));
    }

    #[test]
    fn rule_matches_and_one_fails() {
        let r = req(
            "INBOX",
            Some("boss@corp.com"),
            Some("Hello"),
            None,
            None,
            None,
        );
        let conds = serde_json::json!([
            {"field": "from", "op": "contains", "value": "corp.com"},
            {"field": "subject", "op": "contains", "value": "invoice"}
        ]);
        assert!(!rule_matches(&conds, "and", &r));
    }

    #[test]
    fn rule_matches_or_first_matches() {
        let r = req(
            "INBOX",
            Some("boss@corp.com"),
            Some("Hello"),
            None,
            None,
            None,
        );
        let conds = serde_json::json!([
            {"field": "from", "op": "contains", "value": "corp.com"},
            {"field": "subject", "op": "contains", "value": "invoice"}
        ]);
        assert!(rule_matches(&conds, "or", &r));
    }

    #[test]
    fn rule_matches_or_none_match() {
        let r = req(
            "INBOX",
            Some("other@example.com"),
            Some("Hello"),
            None,
            None,
            None,
        );
        let conds = serde_json::json!([
            {"field": "from", "op": "contains", "value": "corp.com"},
            {"field": "subject", "op": "contains", "value": "invoice"}
        ]);
        assert!(!rule_matches(&conds, "or", &r));
    }

    #[test]
    fn rule_matches_single_cond_match() {
        let r = req("Spam", None, None, None, None, None);
        assert!(rule_matches(&cond("folder", "equals", "spam"), "and", &r));
    }

    #[test]
    fn rule_matches_single_cond_no_match() {
        let r = req("INBOX", None, None, None, None, None);
        assert!(!rule_matches(&cond("folder", "equals", "spam"), "and", &r));
    }

    // ── service constants ─────────────────────────────────────────────────────

    #[test]
    fn service_name_is_expresso_flows() {
        assert_eq!(SERVICE, "expresso-flows");
    }

    #[test]
    fn default_port_is_8005() {
        assert_eq!(DEFAULT_PORT, 8005);
    }

    #[test]
    fn resolve_addr_default_port_when_env_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("PORT");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), DEFAULT_PORT);
    }
}

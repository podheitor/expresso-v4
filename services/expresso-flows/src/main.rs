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
//! CRUD: GET/POST/PATCH/DELETE /api/v1/flows/rules  (JWT auth)
//! Trigger: POST /internal/process                  (internal, no auth)
//!
//! Port: :8005

use std::{env, net::SocketAddr, sync::Arc};

use axum::{
    extract::{FromRequestParts, Path, Request, State},
    http::request::Parts,
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, patch, post},
    Json, Router,
};
use axum::{async_trait, http::StatusCode};
use expresso_auth_client::{AuthContext, Authenticated, AuthRejection, OidcConfig, OidcValidator};
use expresso_core::{begin_tenant_tx, create_db_pool, init_tracing, run_migrations, AppConfig};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::info;
use uuid::Uuid;

const SERVICE:      &str = "expresso-flows";
const DEFAULT_PORT: u16  = 8005;

// ─── App state ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    db:        expresso_core::DbPool,
    validator: Option<Arc<OidcValidator>>,
}

// ─── DTOs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct FlowRule {
    pub id:         Uuid,
    pub user_id:    Uuid,
    pub tenant_id:  Uuid,
    pub name:       String,
    pub enabled:    bool,
    pub priority:   i32,
    pub conditions: serde_json::Value,
    pub actions:    serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct CreateRuleRequest {
    pub name:       Option<String>,
    pub enabled:    Option<bool>,
    pub priority:   Option<i32>,
    pub conditions: serde_json::Value,
    pub actions:    serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct UpdateRuleRequest {
    pub name:       Option<String>,
    pub enabled:    Option<bool>,
    pub priority:   Option<i32>,
    pub conditions: Option<serde_json::Value>,
    pub actions:    Option<serde_json::Value>,
}

/// Payload from expresso-mail: metadata about the delivered message.
#[derive(Debug, Deserialize)]
struct ProcessRequest {
    pub user_id:         Uuid,
    pub tenant_id:       Uuid,
    pub message_id:      Uuid,
    pub folder:          String,
    pub from_addr:       Option<String>,
    pub to_addrs:        Option<Vec<String>>,
    pub subject:         Option<String>,
    pub has_attachments: Option<bool>,
    pub size_bytes:      Option<i32>,
}

#[derive(Debug, Serialize)]
struct ProcessResponse {
    pub matched_rules: usize,
    pub actions:       Vec<serde_json::Value>,
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

// ─── CRUD handlers ────────────────────────────────────────────────────────────

async fn list_rules(
    State(st): State<AppState>,
    AuthCtx(ctx): AuthCtx,
) -> Result<Json<Vec<FlowRule>>, (StatusCode, Json<serde_json::Value>)> {
    let rows: Vec<FlowRule> = sqlx::query_as(
        "SELECT id, user_id, tenant_id, name, enabled, priority, conditions, actions \
         FROM flow_rules \
         WHERE tenant_id = $1 AND user_id = $2 \
         ORDER BY priority ASC, created_at ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(rows))
}

async fn create_rule(
    State(st):   State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Json(req):   Json<CreateRuleRequest>,
) -> Result<(StatusCode, Json<FlowRule>), (StatusCode, Json<serde_json::Value>)> {
    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let rule: FlowRule = sqlx::query_as(
        "INSERT INTO flow_rules (user_id, tenant_id, name, enabled, priority, conditions, actions) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         RETURNING id, user_id, tenant_id, name, enabled, priority, conditions, actions",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(req.name.unwrap_or_default())
    .bind(req.enabled.unwrap_or(true))
    .bind(req.priority.unwrap_or(10))
    .bind(&req.conditions)
    .bind(&req.actions)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tx.commit().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok((StatusCode::CREATED, Json(rule)))
}

async fn update_rule(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id):     Path<Uuid>,
    Json(req):    Json<UpdateRuleRequest>,
) -> Result<Json<FlowRule>, (StatusCode, Json<serde_json::Value>)> {
    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let rule: Option<FlowRule> = sqlx::query_as(
        "UPDATE flow_rules \
         SET name       = COALESCE($3, name), \
             enabled    = COALESCE($4, enabled), \
             priority   = COALESCE($5, priority), \
             conditions = COALESCE($6, conditions), \
             actions    = COALESCE($7, actions), \
             updated_at = NOW() \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $8 \
         RETURNING id, user_id, tenant_id, name, enabled, priority, conditions, actions",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(req.name)
    .bind(req.enabled)
    .bind(req.priority)
    .bind(req.conditions)
    .bind(req.actions)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tx.commit().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    match rule {
        Some(r) => Ok(Json(r)),
        None    => Err((StatusCode::NOT_FOUND, Json(json!({"error": "rule not found"})))),
    }
}

async fn get_rule(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<FlowRule>, (StatusCode, Json<serde_json::Value>)> {
    let row: Option<FlowRule> = sqlx::query_as(
        "SELECT id, user_id, tenant_id, name, enabled, priority, conditions, actions \
         FROM flow_rules \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&st.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    match row {
        Some(r) => Ok(Json(r)),
        None    => Err((StatusCode::NOT_FOUND, Json(json!({"error": "rule not found"})))),
    }
}

async fn delete_rule(
    State(st):    State<AppState>,
    AuthCtx(ctx): AuthCtx,
    Path(id):     Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let mut tx = begin_tenant_tx(&st.db, ctx.tenant_id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    sqlx::query("DELETE FROM flow_rules WHERE id = $1 AND tenant_id = $2 AND user_id = $3")
        .bind(id)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    tx.commit().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Internal process handler ─────────────────────────────────────────────────

/// POST /internal/process — evaluate rules for a freshly delivered message.
/// Returns the list of actions from all matching rules (caller executes them).
async fn internal_process(
    State(st):   State<AppState>,
    Json(req):   Json<ProcessRequest>,
) -> Json<ProcessResponse> {
    let rules: Vec<FlowRule> = match sqlx::query_as(
        "SELECT id, user_id, tenant_id, name, enabled, priority, conditions, actions \
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
        if rule_matches(&rule.conditions, &req) {
            matched += 1;
            if let Some(arr) = rule.actions.as_array() {
                for a in arr {
                    let mut entry = a.clone();
                    entry["rule_id"]      = json!(rule.id);
                    entry["message_id"]   = json!(req.message_id);
                    actions.push(entry);
                }
            }
        }
    }

    Json(ProcessResponse { matched_rules: matched, actions })
}

/// Evaluate all conditions for a rule (AND semantics — all must match).
fn rule_matches(conditions: &serde_json::Value, req: &ProcessRequest) -> bool {
    let conds = match conditions.as_array() {
        Some(a) if !a.is_empty() => a,
        _ => return true, // no conditions = always match
    };

    for cond in conds {
        let field = cond.get("field").and_then(|v| v.as_str()).unwrap_or("");
        let op    = cond.get("op").and_then(|v| v.as_str()).unwrap_or("contains");
        let val   = cond.get("value").and_then(|v| v.as_str()).unwrap_or("");

        let haystack: Option<&str> = match field {
            "from"    => req.from_addr.as_deref(),
            "subject" => req.subject.as_deref(),
            "folder"  => Some(req.folder.as_str()),
            "to"      => {
                // Match if any to_addr satisfies the condition.
                let addrs = req.to_addrs.as_deref().unwrap_or(&[]);
                let matched = addrs.iter().any(|a| str_op(a, op, val));
                if !matched { return false; }
                continue;
            }
            "has_attachment" => {
                // val: "true" | "false"; op is ignored (always equality check)
                let want = val.eq_ignore_ascii_case("true");
                let has  = req.has_attachments.unwrap_or(false);
                if has != want { return false; }
                continue;
            }
            "size" => {
                // op: "gt" | "lt" | "gte" | "lte"; val: bytes as string
                let threshold = val.trim().parse::<i32>().unwrap_or(0);
                let actual    = req.size_bytes.unwrap_or(0);
                let ok = match op {
                    "gt"  | "greater_than"          => actual > threshold,
                    "lt"  | "less_than"             => actual < threshold,
                    "gte" | "greater_than_or_equal" => actual >= threshold,
                    "lte" | "less_than_or_equal"    => actual <= threshold,
                    _                               => actual == threshold,
                };
                if !ok { return false; }
                continue;
            }
            _ => None,
        };

        let hay = match haystack {
            Some(h) => h,
            None    => return false,
        };

        if !str_op(hay, op, val) {
            return false;
        }
    }

    true
}

fn str_op(hay: &str, op: &str, needle: &str) -> bool {
    let hay_lc    = hay.to_lowercase();
    let needle_lc = needle.to_lowercase();
    match op {
        "equals"      => hay_lc == needle_lc,
        "starts_with" => hay_lc.starts_with(&needle_lc),
        _             => hay_lc.contains(&needle_lc), // "contains" is default
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
        Err(e) => { tracing::warn!(error = %e, "OIDC init failed — no JWT auth"); None }
    }
}

fn resolve_addr() -> anyhow::Result<SocketAddr> {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT")
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

    let validator = maybe_build_validator().await;
    let state = AppState { db, validator };

    let app = Router::new()
        .route("/health",                  get(health))
        .route("/ready",                   get(ready))
        .route("/internal/process",        post(internal_process))
        .route("/api/v1/flows/rules",      get(list_rules).post(create_rule))
        .route("/api/v1/flows/rules/:id",  get(get_rule).patch(update_rule).delete(delete_rule))
        .merge(expresso_observability::metrics_router())
        .layer(middleware::from_fn_with_state(state.clone(), inject_validator))
        .with_state(state);

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(service = SERVICE, %addr, "listening");
    axum::serve(listener, app).await?;

    Ok(())
}

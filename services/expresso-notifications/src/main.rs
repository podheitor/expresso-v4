//! expresso-notifications — SSE push for real-time new-mail alerts.
//!
//! Architecture:
//!   expresso-mail   → POST /internal/notify (internal LAN only, no auth)
//!   browser/client  → GET  /notifications/stream (Bearer JWT or Cookie expresso_at)
//!
//! Auth on /notifications/stream:
//!   When AUTH__OIDC_ISSUER + AUTH__OIDC_AUDIENCE are set, the endpoint
//!   validates the JWT and derives user_id/tenant_id from claims.
//!   Without those env vars (dev only), query params user_id/tenant_id are
//!   accepted unauthenticated.
//!
//! In-process broadcast: all SSE streams for a pod share a single
//! `tokio::sync::broadcast` channel; each stream filters by (user_id, tenant_id).
//!
//! Cross-pod (multi-pod deployments) via Redis pub/sub on "expresso:notifications":
//!   - Subscriber side: background task subscribes and rebroadcasts into the
//!     in-process channel, so every pod sees every event published by any pod.
//!   - Publisher side: POST /internal/notify also publishes to Redis so remote
//!     pods receive the event via their subscriber relay.
//!   Both sides activate only when REDIS_URL is set.
//!
//! Ports:
//!   :8006  HTTP (configurable via HOST/PORT)

use std::{env, net::SocketAddr, sync::Arc, time::Duration};

use once_cell::sync::Lazy;
use prometheus::{register_int_counter_vec, IntCounterVec};
use expresso_core::{DbPool, create_db_pool};
use std::collections::HashMap;

static NOTIFICATIONS_DISPATCHED: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "notifications_dispatched_total",
        "Total notifications dispatched, by kind",
        &["kind"]
    )
    .expect("metric registration failed")
});

use axum::{
    async_trait,
    extract::{FromRequestParts, Path, Query, Request, State},
    http::{request::Parts, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::{delete, get, patch, post},
    Json, Router,
};
use time::OffsetDateTime;
use expresso_auth_client::{AuthContext, Authenticated, OidcConfig, OidcValidator};
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast;
use tracing::{info, warn};
use uuid::Uuid;

const SERVICE:      &str  = "expresso-notifications";
const DEFAULT_PORT: u16   = 8006;
const CHANNEL_CAP:  usize = 4096;

// ─── Notification event ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    /// Event kind: "new_mail" | "flags_changed" | "folder_updated"
    pub kind:      String,
    pub user_id:   Uuid,
    pub tenant_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder:    Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Uuid>,
}

// ─── App state ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    tx:         Arc<broadcast::Sender<Notification>>,
    validator:  Option<Arc<OidcValidator>>,
    /// Redis connection pool for cross-pod publish. None when REDIS_URL is unset.
    redis_pub:  Option<Arc<deadpool_redis::Pool>>,
    /// External webhook: (url, client). None when NOTIFICATIONS__WEBHOOK_URL is unset.
    webhook:    Option<(Arc<str>, reqwest::Client)>,
    /// PostgreSQL pool for persisting and querying notifications. None when DATABASE__URL is unset.
    db:         Option<Arc<DbPool>>,
}

// ─── Optional auth extractor ─────────────────────────────────────────────────

/// Tries to extract `Authenticated`; returns `None` if the token is absent or
/// the validator is not in extensions (dev mode). Returns `Some(AuthContext)`
/// on a valid token, and propagates actual auth errors as a 401/403 response.
struct MaybeAuthenticated(Option<AuthContext>);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for MaybeAuthenticated {
    type Rejection = expresso_auth_client::AuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        use std::sync::Arc;
        // If no validator is in extensions → dev mode, skip auth.
        if parts.extensions.get::<Arc<OidcValidator>>().is_none() {
            return Ok(MaybeAuthenticated(None));
        }
        match Authenticated::from_request_parts(parts, state).await {
            Ok(Authenticated(ctx)) => Ok(MaybeAuthenticated(Some(ctx))),
            Err(e) => Err(e),
        }
    }
}

// ─── Auth injection middleware ────────────────────────────────────────────────

/// Injects `Arc<OidcValidator>` into request extensions so that the
/// `Authenticated` extractor can find it.
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

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// POST /internal/notify — called by expresso-mail on new delivery.
/// This endpoint must be network-isolated (not exposed to the internet).
async fn internal_notify(
    State(st):   State<AppState>,
    Json(notif): Json<Notification>,
) -> Json<serde_json::Value> {
    // Broadcast to local SSE streams on this pod.
    let _ = st.tx.send(notif.clone());

    NOTIFICATIONS_DISPATCHED.with_label_values(&[&notif.kind]).inc();

    // Publish to Redis so other pods pick it up via their subscriber relay.
    if let Some(pool) = &st.redis_pub {
        if let Ok(payload) = serde_json::to_string(&notif) {
            match pool.get().await {
                Ok(mut conn) => {
                    use deadpool_redis::redis::AsyncCommands;
                    if let Err(e) = conn.publish::<_, _, ()>("expresso:notifications", &payload).await {
                        warn!(error = %e, "Redis publish failed");
                    }
                }
                Err(e) => warn!(error = %e, "Redis pool get failed for publish"),
            }
        }
    }

    // Fire external webhook with retry (3 attempts, exponential backoff 1s/2s/4s).
    // On exhaustion, payload is written to notification_dlq for inspection/replay.
    if let Some((url, client)) = &st.webhook {
        let url    = url.clone();
        let client = client.clone();
        let body   = serde_json::to_value(&notif).unwrap_or_default();
        let db_dlq = st.db.clone();
        let notif2 = notif.clone();
        tokio::spawn(async move {
            const MAX_ATTEMPTS: u32 = 3;
            let mut last_err = String::new();
            for attempt in 0..MAX_ATTEMPTS {
                if attempt > 0 {
                    tokio::time::sleep(std::time::Duration::from_secs(1u64 << (attempt - 1))).await;
                }
                match client.post(url.as_ref()).json(&body).send().await {
                    Ok(resp) if resp.status().is_success() => return,
                    Ok(resp) => last_err = format!("HTTP {}", resp.status()),
                    Err(e)   => last_err = e.to_string(),
                }
                warn!(attempt = attempt + 1, error = %last_err, "notification webhook attempt failed");
            }
            warn!(error = %last_err, "notification webhook exhausted retries → DLQ");
            if let Some(pool) = db_dlq {
                let payload = serde_json::to_value(&notif2).unwrap_or_default();
                if let Err(e) = sqlx::query(
                    "INSERT INTO notification_dlq \
                        (tenant_id, user_id, kind, payload, attempts, last_error) \
                     VALUES ($1, $2, $3, $4, $5, $6)"
                )
                .bind(notif2.tenant_id)
                .bind(notif2.user_id)
                .bind(&notif2.kind)
                .bind(&payload)
                .bind(MAX_ATTEMPTS as i32)
                .bind(&last_err)
                .execute(pool.as_ref())
                .await {
                    warn!(error = %e, "failed to write to notification_dlq");
                }
            }
        });
    }

    // Persist to DB for digest/history queries.
    if let Some(pool) = &st.db {
        let pool = pool.clone();
        let notif2 = notif.clone();
        tokio::spawn(async move {
            if let Err(e) = sqlx::query(
                "INSERT INTO notifications (tenant_id, user_id, kind, folder, message_id) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(notif2.tenant_id)
            .bind(notif2.user_id)
            .bind(&notif2.kind)
            .bind(&notif2.folder)
            .bind(notif2.message_id)
            .execute(pool.as_ref())
            .await {
                warn!(error = %e, "failed to persist notification");
            }
        });
    }

    Json(json!({"ok": true}))
}

#[derive(Debug, Deserialize)]
struct StreamParams {
    /// Only used in dev mode (no JWT validator configured).
    user_id:   Option<Uuid>,
    tenant_id: Option<Uuid>,
}

/// GET /notifications/stream
///
/// When a JWT validator is configured, the token (Bearer or Cookie) is
/// validated and user_id/tenant_id are taken from the claims.
/// In dev mode (no validator), query params user_id/tenant_id are required.
async fn notifications_stream(
    State(st):     State<AppState>,
    MaybeAuthenticated(auth): MaybeAuthenticated,
    Query(params): Query<StreamParams>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let (user_id, tenant_id) = if st.validator.is_some() {
        match auth {
            Some(ctx) => (ctx.user_id, ctx.tenant_id),
            None => return Err((
                axum::http::StatusCode::UNAUTHORIZED,
                Json(json!({"error": "unauthorized", "message": "missing_bearer"})),
            )),
        }
    } else {
        match (params.user_id, params.tenant_id) {
            (Some(u), Some(t)) => (u, t),
            _ => return Err((
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error": "bad_request", "message": "user_id and tenant_id required in dev mode"})),
            )),
        }
    };

    let rx = st.tx.subscribe();

    let stream = stream::unfold(rx, move |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(notif) => {
                    if notif.user_id != user_id || notif.tenant_id != tenant_id {
                        continue;
                    }
                    let data = serde_json::to_string(&notif).unwrap_or_default();
                    let event = Event::default()
                        .event(&notif.kind)
                        .data(data);
                    return Some((Ok::<Event, std::convert::Infallible>(event), rx));
                }
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(lagged = n, user = %user_id, "SSE channel lagged");
                    let event = Event::default()
                        .event("reconnect")
                        .data(format!("{{\"lagged\":{n}}}"));
                    return Some((Ok(event), rx));
                }
            }
        }
    });

    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(25))
            .text("ping"),
    ))
}

#[derive(Debug, Deserialize)]
struct DigestParams {
    /// RFC 3339 timestamp — aggregate unread notifications since this point.
    since: String,
    /// Only used in dev mode (no validator). Ignored when JWT is present.
    user_id:   Option<Uuid>,
    tenant_id: Option<Uuid>,
}

/// GET /api/v1/notifications/digest?since=<rfc3339>
///
/// Returns counts of unread notifications grouped by kind since the given
/// timestamp. Useful for badge counts and "what did I miss?" summaries.
async fn digest(
    State(st):     State<AppState>,
    MaybeAuthenticated(auth): MaybeAuthenticated,
    Query(params): Query<DigestParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let (user_id, tenant_id) = if st.validator.is_some() {
        match auth {
            Some(ctx) => (ctx.user_id, ctx.tenant_id),
            None => return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "unauthorized", "message": "missing_bearer"})),
            )),
        }
    } else {
        match (params.user_id, params.tenant_id) {
            (Some(u), Some(t)) => (u, t),
            _ => return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "bad_request", "message": "user_id and tenant_id required in dev mode"})),
            )),
        }
    };

    let since = OffsetDateTime::parse(&params.since, &time::format_description::well_known::Rfc3339)
        .map_err(|_| (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "bad_request", "message": "since must be RFC 3339"})),
        ))?;

    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable", "message": "database not configured"})),
    ))?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, COUNT(*)::BIGINT \
         FROM notifications \
         WHERE tenant_id = $1 AND user_id = $2 AND is_read = false AND created_at >= $3 \
         GROUP BY kind \
         ORDER BY COUNT(*) DESC",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(since)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "internal", "message": e.to_string()})),
    ))?;

    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let by_kind: HashMap<&str, i64> = rows.iter().map(|(k, c)| (k.as_str(), *c)).collect();

    Ok(Json(json!({ "total": total, "by_kind": by_kind })))
}

/// Helper: resolve (user_id, tenant_id) from auth or query params (dev mode).
fn resolve_identity(
    st:     &AppState,
    auth:   Option<AuthContext>,
    user_q: Option<Uuid>,
    ten_q:  Option<Uuid>,
) -> Result<(Uuid, Uuid), (StatusCode, Json<serde_json::Value>)> {
    if st.validator.is_some() {
        match auth {
            Some(ctx) => Ok((ctx.user_id, ctx.tenant_id)),
            None => Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "unauthorized", "message": "missing_bearer"})),
            )),
        }
    } else {
        match (user_q, ten_q) {
            (Some(u), Some(t)) => Ok((u, t)),
            _ => Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "bad_request", "message": "user_id and tenant_id required in dev mode"})),
            )),
        }
    }
}

#[derive(Debug, Deserialize)]
struct IdentityParams {
    user_id:   Option<Uuid>,
    tenant_id: Option<Uuid>,
}

/// PATCH /api/v1/notifications/:id/read — mark a single notification as read.
async fn mark_read(
    State(st):     State<AppState>,
    MaybeAuthenticated(auth): MaybeAuthenticated,
    Query(params): Query<IdentityParams>,
    Path(id):      Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let (user_id, tenant_id) = resolve_identity(&st, auth, params.user_id, params.tenant_id)?;
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable", "message": "database not configured"})),
    ))?;
    sqlx::query(
        "UPDATE notifications SET is_read = true \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(pool.as_ref())
    .await
    .map_err(|e| (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "internal", "message": e.to_string()})),
    ))?;
    Ok(StatusCode::NO_CONTENT)
}

// ─── Push subscription (WebPush VAPID) ───────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PushSubscribeBody {
    endpoint: String,
    p256dh:   String,
    auth:     String,
}

/// POST /api/v1/notifications/push — register a WebPush subscription.
async fn push_subscribe(
    State(st):     State<AppState>,
    MaybeAuthenticated(auth): MaybeAuthenticated,
    Query(params): Query<IdentityParams>,
    Json(body):    Json<PushSubscribeBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let (user_id, tenant_id) = resolve_identity(&st, auth, params.user_id, params.tenant_id)?;

    if body.endpoint.is_empty() || body.p256dh.is_empty() || body.auth.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "bad_request", "message": "endpoint, p256dh and auth are required"})),
        ));
    }

    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable", "message": "database not configured"})),
    ))?;

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO notification_push_subscriptions (tenant_id, user_id, endpoint, p256dh, auth) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (tenant_id, user_id, endpoint) DO UPDATE \
             SET p256dh = EXCLUDED.p256dh, auth = EXCLUDED.auth \
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(&body.endpoint)
    .bind(&body.p256dh)
    .bind(&body.auth)
    .fetch_one(pool.as_ref())
    .await
    .map_err(|e| (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "internal", "message": e.to_string()})),
    ))?;

    Ok(Json(json!({"id": id, "ok": true})))
}

#[derive(Debug, Deserialize)]
struct PushUnsubscribeBody {
    endpoint: String,
}

/// DELETE /api/v1/notifications/push — remove a WebPush subscription.
async fn push_unsubscribe(
    State(st):     State<AppState>,
    MaybeAuthenticated(auth): MaybeAuthenticated,
    Query(params): Query<IdentityParams>,
    Json(body):    Json<PushUnsubscribeBody>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let (user_id, tenant_id) = resolve_identity(&st, auth, params.user_id, params.tenant_id)?;

    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable", "message": "database not configured"})),
    ))?;

    sqlx::query(
        "DELETE FROM notification_push_subscriptions \
         WHERE tenant_id = $1 AND user_id = $2 AND endpoint = $3",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(&body.endpoint)
    .execute(pool.as_ref())
    .await
    .map_err(|e| (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "internal", "message": e.to_string()})),
    ))?;

    Ok(StatusCode::NO_CONTENT)
}

/// PATCH /api/v1/notifications/read-all — mark all unread notifications as read.
async fn mark_all_read(
    State(st):     State<AppState>,
    MaybeAuthenticated(auth): MaybeAuthenticated,
    Query(params): Query<IdentityParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let (user_id, tenant_id) = resolve_identity(&st, auth, params.user_id, params.tenant_id)?;
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable", "message": "database not configured"})),
    ))?;
    let r = sqlx::query(
        "UPDATE notifications SET is_read = true \
         WHERE tenant_id = $1 AND user_id = $2 AND is_read = false",
    )
    .bind(tenant_id)
    .bind(user_id)
    .execute(pool.as_ref())
    .await
    .map_err(|e| (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "internal", "message": e.to_string()})),
    ))?;
    Ok(Json(json!({ "marked_read": r.rows_affected() })))
}

/// GET /api/v1/notifications/dlq/stats — contagem agregada por kind + tenant na DLQ.
///
/// Retorna `{total, by_kind: [{kind, count}], by_tenant: [{tenant_id, count}]}`
/// ordenados por count DESC. Útil pra diagnóstico: "que tipos de eventos estão
/// acumulando na DLQ e de quais tenants?". Endpoint ops — sem tenant filter. Sprint #599.
#[derive(Debug, Deserialize)]
struct DlqStatsQuery {
    /// RFC3339 lower bound on failed_at (inclusive). Sprint #619.
    since: Option<String>,
    /// RFC3339 upper bound on failed_at (exclusive). Sprint #619.
    until: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StatsLimitQuery {
    limit: Option<i64>,
}

/// GET /api/v1/notifications/dlq/stats?since=&until=
///
/// Aggregate counts across all DLQ entries: total, breakdown by kind, breakdown
/// by tenant_id. `since` (RFC3339, inclusive) and `until` (RFC3339, exclusive)
/// are optional temporal filters on `failed_at`. Without them, stats cover all
/// entries. Sprints #599 (base) + #619 (temporal filter).
async fn dlq_stats(
    State(st):   State<AppState>,
    Query(q):    Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    // Parse temporal bounds.
    let since_dt = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "since must be RFC3339"}))))
    }).transpose()?;
    let until_dt = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "until must be RFC3339"}))))
    }).transpose()?;

    // Build temporal WHERE clause (shared across all 3 queries).
    let (where_clause, has_since, has_until) = match (since_dt.is_some(), until_dt.is_some()) {
        (true,  true)  => ("WHERE failed_at >= $1::timestamptz AND failed_at < $2::timestamptz", true, true),
        (true,  false) => ("WHERE failed_at >= $1::timestamptz", true, false),
        (false, true)  => ("WHERE failed_at < $1::timestamptz", false, true),
        (false, false) => ("", false, false),
    };

    // Helper: bind temporal params in consistent order.
    macro_rules! bind_temporal {
        ($q:expr) => {{
            let mut qb = $q;
            if has_since { qb = qb.bind(since_dt.unwrap()); }
            if has_until { qb = qb.bind(until_dt.unwrap()); }
            qb
        }};
    }

    let count_sql  = format!("SELECT COUNT(*) FROM notification_dlq {where_clause}");
    let kind_sql   = format!(
        "SELECT kind, COUNT(*)::BIGINT FROM notification_dlq {where_clause} \
         GROUP BY kind ORDER BY COUNT(*) DESC, kind ASC"
    );
    let tenant_sql = format!(
        "SELECT tenant_id, COUNT(*)::BIGINT FROM notification_dlq {where_clause} \
         GROUP BY tenant_id ORDER BY COUNT(*) DESC"
    );

    let (count,): (i64,) = bind_temporal!(sqlx::query_as(&count_sql))
        .fetch_one(pool.as_ref())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let kind_rows: Vec<(String, i64)> = bind_temporal!(sqlx::query_as(&kind_sql))
        .fetch_all(pool.as_ref())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let tenant_rows: Vec<(Option<uuid::Uuid>, i64)> = bind_temporal!(sqlx::query_as(&tenant_sql))
        .fetch_all(pool.as_ref())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let by_kind: Vec<serde_json::Value> = kind_rows.into_iter()
        .map(|(kind, cnt)| json!({"kind": kind, "count": cnt}))
        .collect();
    let by_tenant: Vec<serde_json::Value> = tenant_rows.into_iter()
        .map(|(tid, cnt)| json!({"tenant_id": tid, "count": cnt}))
        .collect();

    Ok(Json(json!({
        "total":     count,
        "by_kind":   by_kind,
        "by_tenant": by_tenant,
        "filter": {"since": q.since, "until": q.until},
    })))
}

/// GET /api/v1/notifications/dlq/stats/by-day?since=&until= — falhas por dia.
///
/// Agrupa entradas da DLQ por `DATE_TRUNC('day', failed_at AT TIME ZONE 'UTC')` e
/// retorna `{days:[{day,count}]}` ordenado ASC. `since`/`until` RFC3339 opcionais.
/// Útil pra timeline de erros: "quantas falhas caíram na DLQ por dia?".
/// Sprint #650.
async fn dlq_stats_by_day(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since_dt = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "since must be RFC3339"}))))
    }).transpose()?;
    let until_dt = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "until must be RFC3339"}))))
    }).transpose()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day \
         ORDER BY day ASC",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let days: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, count)| json!({"day": day, "count": count}))
        .collect();

    Ok(Json(json!({"days": days})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour?since=&until= — falhas DLQ por hora.
///
/// DATE_TRUNC('hour', failed_at) GROUP BY hora. Granularidade intra-dia para identificar
/// picos e janelas de falha. Retorna `{hours:[{hour,count}]}` ordenado ASC.
/// `since`/`until` RFC3339 opcionais. Sprint #700.
async fn dlq_stats_by_hour(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since_dt = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "since must be RFC3339"}))))
    }).transpose()?;
    let until_dt = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "until must be RFC3339"}))))
    }).transpose()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('hour', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:00:00\"Z\"') AS hour, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour \
         ORDER BY hour ASC",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let hours: Vec<serde_json::Value> = rows.into_iter()
        .map(|(hour, count)| json!({"hour": hour, "count": count}))
        .collect();

    Ok(Json(json!({"hours": hours})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-kind?since=&until= — DLQ por (hour, kind).
///
/// GROUP BY (DATE_TRUNC('hour', failed_at), kind) ORDER BY hour ASC, kind ASC.
/// Granularidade intra-dia por tipo de notificação. Análogo a by-hour (#700) escopado por kind.
/// Retorna `{rows:[{hour,kind,count}]}`. Sprint #720.
async fn dlq_stats_by_hour_and_kind(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since_dt = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "since must be RFC3339"}))))
    }).transpose()?;
    let until_dt = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "until must be RFC3339"}))))
    }).transpose()?;

    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('hour', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:00:00\"Z\"') AS hour, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour, kind \
         ORDER BY hour ASC, kind ASC",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(hour, kind, count)| json!({"hour": hour, "kind": kind, "count": count}))
        .collect();

    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-kind-and-day?since=&until= — falhas DLQ por dia e kind.
///
/// Agrupa `notification_dlq` por `(DATE_TRUNC('day', failed_at), kind)` e retorna
/// `{rows:[{day,kind,count}]}` ordenado `(day ASC, kind ASC)`. `since`/`until` RFC3339
/// opcionais com o padrão `$N::timestamptz IS NULL OR ...`. Sprint #656.
async fn dlq_stats_by_kind_and_day(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since_dt = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "since must be RFC3339"}))))
    }).transpose()?;
    let until_dt = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "until must be RFC3339"}))))
    }).transpose()?;

    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, kind \
         ORDER BY day ASC, kind ASC",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, kind, count)| json!({"day": day, "kind": kind, "count": count}))
        .collect();

    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-tenant-and-day?since=&until= — falhas DLQ por dia e tenant.
///
/// Agrupa `notification_dlq` por `(DATE_TRUNC('day', failed_at), tenant_id)` e retorna
/// `{rows:[{day,tenant_id,count}]}` ordenado `(day ASC, tenant_id ASC)`. Simétrico com
/// `by-kind-and-day` (#656) mas escopado por tenant. Sprint #661.
async fn dlq_stats_by_tenant_and_day(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since_dt = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "since must be RFC3339"}))))
    }).transpose()?;
    let until_dt = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "until must be RFC3339"}))))
    }).transpose()?;

    let rows: Vec<(String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, tenant_id \
         ORDER BY day ASC, tenant_id ASC",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, tenant_id, count)| json!({"day": day, "tenant_id": tenant_id, "count": count}))
        .collect();

    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-error-kind?since=&until= — DLQ por error_kind.
///
/// Agrupa `notification_dlq` por `last_error` (tratado como kind literal) e retorna
/// `{rows:[{error_kind,count}]}` ordenado `count DESC`. `since`/`until` RFC3339 opcionais.
/// Complementa `by-kind-and-day` (#656) com foco no breakdown por tipo de erro acumulado.
/// Sprint #666.
async fn dlq_stats_by_error_kind(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since_dt = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "since must be RFC3339"}))))
    }).transpose()?;
    let until_dt = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "until must be RFC3339"}))))
    }).transpose()?;

    let rows: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT last_error, COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
            AND ($2::timestamptz IS NULL OR failed_at <  $2) \
          GROUP BY last_error \
          ORDER BY count DESC, last_error ASC NULLS LAST",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(error_kind, count)| json!({"error_kind": error_kind, "count": count}))
        .collect();

    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-tenant?limit=N — DLQ por tenant_id agregado.
///
/// Agrupa `notification_dlq` por `tenant_id` e retorna
/// `{rows:[{tenant_id,count}]}` ordenado `count DESC`. Sem filtro temporal —
/// visão total acumulada. `limit` default 20 max 200. Complementa `by-tenant-and-day`
/// (#661) com rollup simples. Sprint #671.
async fn dlq_stats_by_tenant(
    State(st): State<AppState>,
    Query(q):  Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT tenant_id, COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY tenant_id \
          ORDER BY count DESC, tenant_id ASC NULLS LAST \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(tid, count)| json!({"tenant_id": tid, "count": count}))
        .collect();

    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-tenant-and-kind?limit=N — DLQ por (tenant_id, kind).
///
/// GROUP BY (tenant_id, kind) ORDER BY count DESC. Análogo a by-kind-and-user (#705) mas
/// escopado por tenant. `limit` default 20 max 200. Retorna `{rows:[{tenant_id,kind,count}]}`. Sprint #710.
async fn dlq_stats_by_tenant_and_kind(
    State(st): State<AppState>,
    Query(q):  Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(uuid::Uuid, String, i64)> = sqlx::query_as(
        "SELECT tenant_id, kind, COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY tenant_id, kind \
          ORDER BY count DESC, tenant_id ASC, kind ASC \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(tid, kind, count)| json!({"tenant_id": tid, "kind": kind, "count": count}))
        .collect();

    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-kind?limit=N — DLQ por kind agregado.
///
/// GROUP BY kind ORDER BY count DESC sem breakdown temporal. `limit` default 20 max 200.
/// Rollup simples de by-kind-and-day (#656) — visão total acumulada. Sprint #676.
async fn dlq_stats_by_kind(
    State(st): State<AppState>,
    Query(q):  Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY kind \
          ORDER BY count DESC, kind ASC \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, count)| json!({"kind": kind, "count": count}))
        .collect();

    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-user?since=&until= — DLQ por dia e user_id.
///
/// GROUP BY (DATE_TRUNC('day', failed_at), user_id) retorna
/// `{rows:[{day,user_id,count}]}` ordenado `(day ASC, user_id ASC)`.
/// `since`/`until` RFC3339 opcionais. Análogo a `by-tenant-and-day` (#661) escopado
/// por usuário. Sprint #691.
async fn dlq_stats_by_day_and_user(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since_dt = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "since must be RFC3339"}))))
    }).transpose()?;
    let until_dt = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "until must be RFC3339"}))))
    }).transpose()?;

    let rows: Vec<(String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, user_id \
         ORDER BY day ASC, user_id ASC NULLS LAST",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, user_id, count)| json!({"day": day, "user_id": user_id, "count": count}))
        .collect();

    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-user?limit=N — DLQ por user_id agregado.
///
/// Agrupa `notification_dlq` por `user_id` e retorna
/// `{rows:[{user_id,count}]}` ordenado `count DESC`. Análogo a `by-tenant` (#671)
/// escopado por usuário. `limit` default 20 max 200. Sprint #686.
async fn dlq_stats_by_user(
    State(st): State<AppState>,
    Query(q):  Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT user_id, COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY user_id \
          ORDER BY count DESC, user_id ASC NULLS LAST \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(uid, count)| json!({"user_id": uid, "count": count}))
        .collect();

    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-kind-and-user?limit=N — DLQ por (kind, user_id).
///
/// GROUP BY (kind, user_id) ORDER BY count DESC. Identifica usuários afetados por cada
/// tipo de falha. `limit` default 20 max 200. Retorna `{rows:[{kind,user_id,count}]}`. Sprint #705.
async fn dlq_stats_by_kind_and_user(
    State(st): State<AppState>,
    Query(q):  Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT kind, user_id, COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY kind, user_id \
          ORDER BY count DESC, kind ASC, user_id ASC NULLS LAST \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, user_id, count)| json!({"kind": kind, "user_id": user_id, "count": count}))
        .collect();

    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/attempts-distribution — histograma de attempts.
///
/// Classifica entradas por `attempts`: buckets 1/2/3/4/5+ via COUNT FILTER.
/// Retorna `{buckets:[{attempts,count}]}`. Identifica entradas presas em retry loops.
/// Sprint #681.
async fn dlq_stats_attempts_distribution(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let (a1, a2, a3, a4, a5plus): (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE attempts = 1)::BIGINT, \
            COUNT(*) FILTER (WHERE attempts = 2)::BIGINT, \
            COUNT(*) FILTER (WHERE attempts = 3)::BIGINT, \
            COUNT(*) FILTER (WHERE attempts = 4)::BIGINT, \
            COUNT(*) FILTER (WHERE attempts >= 5)::BIGINT \
           FROM notification_dlq",
    )
    .fetch_one(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({
        "buckets": [
            {"attempts": 1,    "count": a1},
            {"attempts": 2,    "count": a2},
            {"attempts": 3,    "count": a3},
            {"attempts": 4,    "count": a4},
            {"attempts": "5+", "count": a5plus},
        ]
    })))
}

/// GET /api/v1/notifications/dlq/stats/retention?days=N — entries mais velhas que N dias.
///
/// Conta entradas onde `failed_at < NOW() - INTERVAL '$N days'`.
/// Identifica entries esquecidas que deveriam ter sido resolvidas ou expiradas.
/// `days` default 7. Retorna `{days,stale_count,oldest_failed_at}`. Sprint #695.
#[derive(Debug, Deserialize)]
struct RetentionQuery {
    days: Option<i64>,
}

async fn dlq_stats_retention(
    State(st): State<AppState>,
    Query(q):  Query<RetentionQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let days = q.days.unwrap_or(7).max(0);

    let (stale_count, oldest_failed_at): (i64, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT, MIN(failed_at) \
           FROM notification_dlq \
          WHERE failed_at < NOW() - ($1 * INTERVAL '1 day')",
    )
    .bind(days)
    .fetch_one(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({
        "days":             days,
        "stale_count":      stale_count,
        "oldest_failed_at": oldest_failed_at,
    })))
}

/// GET /api/v1/notifications/dlq/stats/by-minute?since=&until= — DLQ por minuto.
///
/// DATE_TRUNC('minute') GROUP BY ORDER BY minute ASC. Granularidade fina para identificar
/// picos em janelas de tempo estreitas. Temporal bounds opcionais.
/// Retorna `{rows:[{minute,count}]}`. Sprint #725.
async fn dlq_stats_by_minute(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since_dt = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "since must be RFC3339"}))))
    }).transpose()?;
    let until_dt = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "until must be RFC3339"}))))
    }).transpose()?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('minute', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:00\"Z\"') AS minute, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute \
         ORDER BY minute ASC",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(minute, count)| json!({"minute": minute, "count": count}))
        .collect();

    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-kind?since=&until= — DLQ por (minute, kind).
///
/// GROUP BY (DATE_TRUNC('minute', failed_at), kind) ORDER BY minute ASC, kind ASC.
/// Granularidade fina por tipo — identifica bursts de um kind específico dentro de minutos.
/// Retorna `{rows:[{minute,kind,count}]}`. Sprint #730.
async fn dlq_stats_by_minute_and_kind(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since_dt = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "since must be RFC3339"}))))
    }).transpose()?;
    let until_dt = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "until must be RFC3339"}))))
    }).transpose()?;

    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('minute', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:00\"Z\"') AS minute, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute, kind \
         ORDER BY minute ASC, kind ASC",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(minute, kind, count)| json!({"minute": minute, "kind": kind, "count": count}))
        .collect();

    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-kind?since=&until= — DLQ por (day, kind).
///
/// GROUP BY (DATE_TRUNC('day', failed_at), kind) ORDER BY day ASC, kind ASC.
/// Análogo a by-tenant-and-day (#661) escopado por kind. Temporal bounds opcionais.
/// Retorna `{rows:[{day,kind,count}]}`. Sprint #715.
async fn dlq_stats_by_day_and_kind(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since_dt = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "since must be RFC3339"}))))
    }).transpose()?;
    let until_dt = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "until must be RFC3339"}))))
    }).transpose()?;

    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, kind \
         ORDER BY day ASC, kind ASC",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, kind, count)| json!({"day": day, "kind": kind, "count": count}))
        .collect();

    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/count — fast count of DLQ entries.
async fn count_dlq(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM notification_dlq")
        .fetch_one(pool.as_ref())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
    Ok(Json(json!({"count": count})))
}

/// GET /api/v1/notifications/dlq/oldest — entry mais antigo da DLQ.
///
/// Retorna o entry com o menor `failed_at` (ORDER BY failed_at ASC LIMIT 1).
/// Útil pra dashboards de "tempo desde primeira falha" sem listar a DLQ inteira.
/// Response: `{entry}` com shape idêntico ao `GET /dlq/:id`, ou `{entry: null}`
/// quando a DLQ está vazia. Sprint #639.
async fn oldest_dlq_entry(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use sqlx::Row as _;
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let row = sqlx::query(
        "SELECT id, tenant_id, user_id, kind, payload, attempts, last_error, failed_at \
           FROM notification_dlq ORDER BY failed_at ASC LIMIT 1",
    )
    .fetch_optional(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let entry = row.map(|r| {
        let id:         Uuid                = r.get("id");
        let tenant_id:  Option<Uuid>        = r.try_get("tenant_id").ok();
        let user_id:    Option<Uuid>        = r.try_get("user_id").ok();
        let kind:       String              = r.get("kind");
        let payload:    serde_json::Value   = r.get("payload");
        let attempts:   i32                 = r.get("attempts");
        let last_error: Option<String>      = r.try_get("last_error").ok();
        let failed_at:  OffsetDateTime      = r.get("failed_at");
        json!({
            "id":         id,
            "tenant_id":  tenant_id,
            "user_id":    user_id,
            "kind":       kind,
            "payload":    payload,
            "attempts":   attempts,
            "last_error": last_error,
            "failed_at":  failed_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        })
    });
    Ok(Json(json!({"entry": entry})))
}

/// GET /api/v1/notifications/dlq/newest — entry mais recente da DLQ.
///
/// Retorna o entry com o maior `failed_at` (ORDER BY failed_at DESC LIMIT 1).
/// Simetria com oldest (#639): juntos formam o par "quando foi a primeira falha /
/// quando foi a última falha" — útil pra alertas "DLQ ainda está recebendo erros".
/// Response: `{entry}` com shape idêntico ao GET /dlq/:id, ou `{entry: null}`
/// quando a DLQ está vazia. Sprint #645.
async fn newest_dlq_entry(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use sqlx::Row as _;
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let row = sqlx::query(
        "SELECT id, tenant_id, user_id, kind, payload, attempts, last_error, failed_at \
           FROM notification_dlq ORDER BY failed_at DESC LIMIT 1",
    )
    .fetch_optional(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let entry = row.map(|r| {
        let id:         Uuid                = r.get("id");
        let tenant_id:  Option<Uuid>        = r.try_get("tenant_id").ok();
        let user_id:    Option<Uuid>        = r.try_get("user_id").ok();
        let kind:       String              = r.get("kind");
        let payload:    serde_json::Value   = r.get("payload");
        let attempts:   i32                 = r.get("attempts");
        let last_error: Option<String>      = r.try_get("last_error").ok();
        let failed_at:  OffsetDateTime      = r.get("failed_at");
        json!({
            "id":         id,
            "tenant_id":  tenant_id,
            "user_id":    user_id,
            "kind":       kind,
            "payload":    payload,
            "attempts":   attempts,
            "last_error": last_error,
            "failed_at":  failed_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        })
    });
    Ok(Json(json!({"entry": entry})))
}

/// GET /api/v1/notifications/dlq?limit=N&offset=N&kind=K&tenant_id=UUID&since=RFC3339&until=RFC3339
///
/// List DLQ entries (newest first). Optional filters: `kind`, `tenant_id`,
/// `since` (failed_at >= since, RFC3339, inclusive), `until` (failed_at < until,
/// RFC3339, exclusive). Filters compose with AND. Limit 1–500, default 50.
/// Sprints #600 (kind+tenant_id) + #614 (since+until temporal filter).
async fn list_dlq(
    State(st):   State<AppState>,
    Query(q):    Query<DlqListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use sqlx::Row as _;
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let limit  = q.limit.unwrap_or(50).clamp(1, 500) as i64;
    let offset = q.offset.unwrap_or(0).max(0) as i64;

    // Parse temporal bounds (RFC3339 → OffsetDateTime).
    let since_dt = q.since.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "since must be RFC3339"}))))
    }).transpose()?;
    let until_dt = q.until.as_deref().map(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({"error": "until must be RFC3339"}))))
    }).transpose()?;

    // Build WHERE conditions; $1=limit, $2=offset always; filters start at $3.
    let mut conditions: Vec<String> = Vec::new();
    let mut next_param = 3usize;

    if q.kind.is_some()      { conditions.push(format!("kind = ${next_param}"));                next_param += 1; }
    if q.tenant_id.is_some() { conditions.push(format!("tenant_id = ${next_param}"));           next_param += 1; }
    if since_dt.is_some()    { conditions.push(format!("failed_at >= ${next_param}::timestamptz")); next_param += 1; }
    if until_dt.is_some()    { conditions.push(format!("failed_at <  ${next_param}::timestamptz")); next_param += 1; }
    let _ = next_param; // suppress "unused" warning after last use

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT id, tenant_id, user_id, kind, payload, attempts, last_error, failed_at \
           FROM notification_dlq \
          {where_clause} \
          ORDER BY failed_at DESC \
          LIMIT $1 OFFSET $2"
    );
    let mut q_builder = sqlx::query(&sql).bind(limit).bind(offset);
    if let Some(ref k)  = q.kind      { q_builder = q_builder.bind(k.as_str()); }
    if let Some(t)      = q.tenant_id { q_builder = q_builder.bind(t); }
    if let Some(s)      = since_dt    { q_builder = q_builder.bind(s); }
    if let Some(u)      = until_dt    { q_builder = q_builder.bind(u); }

    let rows = q_builder
        .fetch_all(pool.as_ref())
        .await
        .map_err(|e| (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "internal", "message": e.to_string()})),
        ))?;
    let items: Vec<serde_json::Value> = rows.iter().map(|r| {
        let id:         Uuid                  = r.get("id");
        let tenant_id:  Option<Uuid>          = r.try_get("tenant_id").ok();
        let user_id:    Option<Uuid>          = r.try_get("user_id").ok();
        let kind:       String                = r.get("kind");
        let payload:    serde_json::Value     = r.get("payload");
        let attempts:   i32                   = r.get("attempts");
        let last_error: Option<String>        = r.try_get("last_error").ok();
        let failed_at:  OffsetDateTime        = r.get("failed_at");
        json!({
            "id":         id,
            "tenant_id":  tenant_id,
            "user_id":    user_id,
            "kind":       kind,
            "payload":    payload,
            "attempts":   attempts,
            "last_error": last_error,
            "failed_at":  failed_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        })
    }).collect();
    Ok(Json(json!({"items": items, "limit": limit, "offset": offset,
        "filter": {"kind": q.kind, "tenant_id": q.tenant_id,
                   "since": q.since, "until": q.until}})))
}

#[derive(Debug, Deserialize)]
struct DlqListQuery {
    limit:     Option<u32>,
    offset:    Option<u32>,
    kind:      Option<String>,
    tenant_id: Option<Uuid>,
    /// RFC3339 lower bound on failed_at (inclusive). Sprint #614.
    since:     Option<String>,
    /// RFC3339 upper bound on failed_at (exclusive). Sprint #614.
    until:     Option<String>,
}

/// GET /api/v1/notifications/dlq/:id — inspeciona uma entrada individual da DLQ.
///
/// Retorna os mesmos campos que o list (id, tenant_id, user_id, kind, payload,
/// attempts, last_error, failed_at). 404 se o entry não existe. Sprint #586.
async fn get_dlq_entry(
    State(st): State<AppState>,
    Path(id):  Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use sqlx::Row as _;
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let row = sqlx::query(
        "SELECT id, tenant_id, user_id, kind, payload, attempts, last_error, failed_at \
           FROM notification_dlq WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))))?;

    let tenant_id:  Option<Uuid>      = row.try_get("tenant_id").ok();
    let user_id:    Option<Uuid>      = row.try_get("user_id").ok();
    let kind:       String            = row.get("kind");
    let payload:    serde_json::Value = row.get("payload");
    let attempts:   i32               = row.get("attempts");
    let last_error: Option<String>    = row.try_get("last_error").ok();
    let failed_at:  OffsetDateTime    = row.get("failed_at");

    Ok(Json(json!({
        "id":         id,
        "tenant_id":  tenant_id,
        "user_id":    user_id,
        "kind":       kind,
        "payload":    payload,
        "attempts":   attempts,
        "last_error": last_error,
        "failed_at":  failed_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
    })))
}

/// DELETE /api/v1/notifications/dlq/:id — remove a DLQ entry (after manual retry).
async fn delete_dlq_entry(
    State(st): State<AppState>,
    Path(id):  Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    sqlx::query("DELETE FROM notification_dlq WHERE id = $1")
        .bind(id)
        .execute(pool.as_ref())
        .await
        .map_err(|e| (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "internal", "message": e.to_string()})),
        ))?;
    Ok(StatusCode::NO_CONTENT)
}

/// PATCH /api/v1/notifications/dlq/:id — ops annotation: atualiza last_error e/ou attempts.
///
/// Body JSON: `{last_error?: string | null, attempts?: integer}`.
/// Útil para operadores anotarem a causa raiz ou resetarem contagem de tentativas
/// sem precisar deletar + re-inserir a entry. 404 se entry não existe.
/// Retorna `{id, updated}` com campos efetivamente alterados. Sprint #593.
#[derive(Debug, Deserialize)]
struct PatchDlqBody {
    last_error: Option<serde_json::Value>, // string | null
    attempts:   Option<i32>,
}

async fn patch_dlq_entry(
    State(st): State<AppState>,
    Path(id):  Path<Uuid>,
    Json(body): Json<PatchDlqBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    // Validate at least one field provided.
    if body.last_error.is_none() && body.attempts.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "at least one of last_error or attempts is required"})),
        ));
    }

    // Verify entry exists first (404 guard).
    let exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM notification_dlq WHERE id = $1")
        .bind(id)
        .fetch_optional(pool.as_ref())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
    if exists.is_none() {
        return Err((StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))));
    }

    let mut updated: Vec<&str> = Vec::new();

    if let Some(ref le) = body.last_error {
        let val: Option<String> = match le {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s.clone()),
            _ => return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "last_error must be a string or null"})))),
        };
        sqlx::query("UPDATE notification_dlq SET last_error = $1 WHERE id = $2")
            .bind(val)
            .bind(id)
            .execute(pool.as_ref())
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
        updated.push("last_error");
    }

    if let Some(att) = body.attempts {
        if att < 0 {
            return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "attempts must be >= 0"}))));
        }
        sqlx::query("UPDATE notification_dlq SET attempts = $1 WHERE id = $2")
            .bind(att)
            .bind(id)
            .execute(pool.as_ref())
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
        updated.push("attempts");
    }

    Ok(Json(json!({"id": id, "updated": updated})))
}

/// PATCH /api/v1/notifications/dlq/bulk — bulk patch multiple DLQ entries by IDs.
///
/// Body: `{ids: [uuid], last_error?: string | null, attempts?: integer}`.
/// At least one of `last_error` or `attempts` must be present.
/// At least one ID must be provided; max 200 IDs per call.
/// Returns `{updated, not_found, ids_updated: [uuid]}`. Sprint #624.
#[derive(Debug, Deserialize)]
struct BulkPatchDlqBody {
    ids:        Vec<Uuid>,
    last_error: Option<serde_json::Value>, // string | null
    attempts:   Option<i32>,
}

async fn bulk_patch_dlq(
    State(st): State<AppState>,
    Json(body): Json<BulkPatchDlqBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.ids.is_empty() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "ids must not be empty"}))));
    }
    if body.ids.len() > 200 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": format!("too many ids: {} (max 200)", body.ids.len())}))));
    }
    if body.last_error.is_none() && body.attempts.is_none() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "at least one of last_error or attempts is required"}))));
    }

    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    // Validate last_error value if present.
    let last_error_val: Option<Option<String>> = match &body.last_error {
        None => None,
        Some(serde_json::Value::Null) => Some(None),
        Some(serde_json::Value::String(s)) => Some(Some(s.clone())),
        Some(_) => return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "last_error must be a string or null"})))),
    };
    if let Some(att) = body.attempts {
        if att < 0 {
            return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "attempts must be >= 0"}))));
        }
    }

    // Apply updates and collect which IDs existed.
    let mut ids_updated: Vec<Uuid> = Vec::new();

    for &id in &body.ids {
        // Check existence first.
        let exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM notification_dlq WHERE id = $1")
            .bind(id)
            .fetch_optional(pool.as_ref())
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
        if exists.is_none() {
            continue;
        }

        if let Some(ref le) = last_error_val {
            sqlx::query("UPDATE notification_dlq SET last_error = $1 WHERE id = $2")
                .bind(le.as_deref())
                .bind(id)
                .execute(pool.as_ref())
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
        }
        if let Some(att) = body.attempts {
            sqlx::query("UPDATE notification_dlq SET attempts = $1 WHERE id = $2")
                .bind(att)
                .bind(id)
                .execute(pool.as_ref())
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
        }
        ids_updated.push(id);
    }

    let not_found = body.ids.len() - ids_updated.len();
    Ok(Json(json!({
        "updated":     ids_updated.len(),
        "not_found":   not_found,
        "ids_updated": ids_updated,
    })))
}

/// POST /api/v1/notifications/dlq/:id/retry — re-dispatch a DLQ entry.
///
/// Reads the saved payload, posts it to /internal/notify on this pod, and
/// removes the DLQ entry on success. On failure the entry remains in the DLQ.
async fn retry_dlq_entry(
    State(st): State<AppState>,
    Path(id):  Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use sqlx::Row as _;
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    // Load the DLQ entry.
    let row = sqlx::query(
        "SELECT id, tenant_id, user_id, kind, payload, folder, message_id \
           FROM notification_dlq \
          WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))))?;

    let payload: serde_json::Value = row.get("payload");

    // Reconstruct the notification from the saved payload.
    let notif: Notification = serde_json::from_value(payload.clone())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    // Re-dispatch: broadcast locally + Redis + webhook (same as internal_notify).
    let _ = st.tx.send(notif.clone());
    NOTIFICATIONS_DISPATCHED.with_label_values(&[&notif.kind]).inc();

    if let Some(redis_pool) = &st.redis_pub {
        if let Ok(body) = serde_json::to_string(&notif) {
            if let Ok(mut conn) = redis_pool.get().await {
                use deadpool_redis::redis::AsyncCommands;
                let _ = conn.publish::<_, _, ()>("expresso:notifications", &body).await;
            }
        }
    }

    if let Some((url, client)) = &st.webhook {
        let url    = url.clone();
        let client = client.clone();
        let body   = payload.clone();
        tokio::spawn(async move {
            let _ = client.post(url.as_ref()).json(&body).send().await;
        });
    }

    // Delete the DLQ entry now that it's been re-dispatched.
    sqlx::query("DELETE FROM notification_dlq WHERE id = $1")
        .bind(id)
        .execute(pool.as_ref())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({"retried": true, "id": id, "kind": notif.kind})))
}

/// DELETE /api/v1/notifications/dlq — purge total da DLQ sem re-despachar.
///
/// Remove todas as entradas da `notification_dlq`. Operação destrutiva —
/// usar apenas quando os eventos falhos são obsoletos e não precisam de retry.
/// Retorna `{deleted}` com a contagem de linhas removidas. Sprint #585.
async fn purge_dlq(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let res = sqlx::query("DELETE FROM notification_dlq")
        .execute(pool.as_ref())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
    Ok(Json(json!({"deleted": res.rows_affected()})))
}

/// POST /api/v1/notifications/dlq/retry-all — re-despacha todos os entries da DLQ.
///
/// Processa cada entry: broadcast SSE + Redis + webhook fire-and-forget + DELETE.
/// Entries onde o payload é inválido (JSON corrompido) são contados como falha e
/// deixados intactos na DLQ. Retorna `{retried, failed, total}` — 200 sempre
/// (best-effort; falhas parciais não abortam o batch).
async fn retry_all_dlq(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use sqlx::Row as _;
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let rows = sqlx::query(
        "SELECT id, kind, payload FROM notification_dlq ORDER BY created_at ASC",
    )
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let total = rows.len() as u64;
    let (mut retried, mut failed) = (0u64, 0u64);

    for row in rows {
        let id:      Uuid                = row.get("id");
        let payload: serde_json::Value   = row.get("payload");

        let notif: Notification = match serde_json::from_value(payload.clone()) {
            Ok(n)  => n,
            Err(_) => { failed += 1; continue; }
        };

        let _ = st.tx.send(notif.clone());
        NOTIFICATIONS_DISPATCHED.with_label_values(&[&notif.kind]).inc();

        if let Some(redis_pool) = &st.redis_pub {
            if let Ok(body) = serde_json::to_string(&notif) {
                if let Ok(mut conn) = redis_pool.get().await {
                    use deadpool_redis::redis::AsyncCommands;
                    let _ = conn.publish::<_, _, ()>("expresso:notifications", &body).await;
                }
            }
        }

        if let Some((url, client)) = &st.webhook {
            let url    = url.clone();
            let client = client.clone();
            let body   = payload.clone();
            tokio::spawn(async move {
                let _ = client.post(url.as_ref()).json(&body).send().await;
            });
        }

        match sqlx::query("DELETE FROM notification_dlq WHERE id = $1")
            .bind(id)
            .execute(pool.as_ref())
            .await
        {
            Ok(_)  => retried += 1,
            Err(_) => failed  += 1,
        }
    }

    Ok(Json(json!({"retried": retried, "failed": failed, "total": total})))
}

/// POST /api/v1/notifications/dlq/retry-filtered?kind=&tenant_id= — re-dispatch filtrado da DLQ.
///
/// Re-despacha apenas os entries que casam com os filtros `kind` e/ou `tenant_id`.
/// Sem filtros, comporta-se como retry-all. Mesmo padrão best-effort do retry-all:
/// falhas parciais não abortam o batch; sempre 200 com {retried, failed, total, filter}.
/// Sprint #606.
async fn retry_filtered_dlq(
    State(st): State<AppState>,
    Query(q):  Query<DlqListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use sqlx::Row as _;
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let (where_clause, bind_kind, bind_tenant) = match (&q.kind, &q.tenant_id) {
        (Some(_), Some(_)) => ("WHERE kind = $1 AND tenant_id = $2", true, true),
        (Some(_), None)    => ("WHERE kind = $1",                    true, false),
        (None,    Some(_)) => ("WHERE tenant_id = $1",               false, true),
        (None,    None)    => ("",                                    false, false),
    };
    let sql = format!(
        "SELECT id, kind, payload FROM notification_dlq {where_clause} ORDER BY created_at ASC"
    );
    let mut q_builder = sqlx::query(&sql);
    if bind_kind   { q_builder = q_builder.bind(q.kind.as_deref().unwrap()); }
    if bind_tenant { q_builder = q_builder.bind(q.tenant_id.unwrap()); }

    let rows = q_builder
        .fetch_all(pool.as_ref())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let total = rows.len() as u64;
    let (mut retried, mut failed) = (0u64, 0u64);

    for row in rows {
        let id:      Uuid              = row.get("id");
        let payload: serde_json::Value = row.get("payload");

        let notif: Notification = match serde_json::from_value(payload.clone()) {
            Ok(n)  => n,
            Err(_) => { failed += 1; continue; }
        };

        let _ = st.tx.send(notif.clone());
        NOTIFICATIONS_DISPATCHED.with_label_values(&[&notif.kind]).inc();

        if let Some(redis_pool) = &st.redis_pub {
            if let Ok(body) = serde_json::to_string(&notif) {
                if let Ok(mut conn) = redis_pool.get().await {
                    use deadpool_redis::redis::AsyncCommands;
                    let _ = conn.publish::<_, _, ()>("expresso:notifications", &body).await;
                }
            }
        }

        if let Some((url, client)) = &st.webhook {
            let url    = url.clone();
            let client = client.clone();
            let body   = payload.clone();
            tokio::spawn(async move {
                let _ = client.post(url.as_ref()).json(&body).send().await;
            });
        }

        match sqlx::query("DELETE FROM notification_dlq WHERE id = $1")
            .bind(id)
            .execute(pool.as_ref())
            .await
        {
            Ok(_)  => retried += 1,
            Err(_) => failed  += 1,
        }
    }

    Ok(Json(json!({
        "retried": retried,
        "failed":  failed,
        "total":   total,
        "filter":  {"kind": q.kind, "tenant_id": q.tenant_id},
    })))
}

/// POST /api/v1/notifications/dlq/bulk/count — count DLQ entries by list of IDs.
///
/// Body: `{ids: [uuid]}`. Returns `{found, not_found, ids_found: [uuid]}`.
/// Useful for UI to verify which IDs are still in the DLQ before bulk retry.
/// Max 200 IDs per call. Does NOT modify any entry. Sprint #634.
#[derive(Debug, Deserialize)]
struct BulkCountDlqBody {
    ids: Vec<Uuid>,
}

async fn bulk_count_dlq(
    State(st): State<AppState>,
    Json(body): Json<BulkCountDlqBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.ids.is_empty() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "ids must not be empty"}))));
    }
    if body.ids.len() > 200 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": format!("too many ids: {} (max 200)", body.ids.len())}))));
    }

    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    // Single query: fetch all ids that exist, preserving input ordering.
    let found_rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM notification_dlq WHERE id = ANY($1)",
    )
    .bind(&body.ids)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let found_set: std::collections::HashSet<Uuid> = found_rows.into_iter().map(|(id,)| id).collect();
    let ids_found: Vec<Uuid> = body.ids.iter().filter(|id| found_set.contains(id)).copied().collect();
    let not_found = body.ids.len() - ids_found.len();

    Ok(Json(json!({
        "found":     ids_found.len(),
        "not_found": not_found,
        "ids_found": ids_found,
    })))
}

/// POST /api/v1/notifications/dlq/bulk/retry — bulk retry por lista de IDs.
///
/// Body: `{ids: [uuid]}`. Re-despacha cada entry: broadcast SSE + Redis + webhook
/// fire-and-forget + DELETE. Entries não encontradas são contadas como `not_found`.
/// Payload inválido (JSON corrompido no `payload`) conta como `failed` e a entry
/// fica intacta na DLQ. Retorna `{retried, failed, not_found, ids_retried: [uuid]}`.
/// Best-effort: falhas parciais não abortam o batch; sempre 200 (exceto validação).
/// Paralelo do bulk-patch #624 mas com re-dispatch + delete. Sprint #629.
#[derive(Debug, Deserialize)]
struct BulkRetryDlqBody {
    ids: Vec<Uuid>,
}

async fn bulk_retry_dlq(
    State(st): State<AppState>,
    Json(body): Json<BulkRetryDlqBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.ids.is_empty() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "ids must not be empty"}))));
    }
    if body.ids.len() > 200 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": format!("too many ids: {} (max 200)", body.ids.len())}))));
    }

    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    use sqlx::Row as _;
    let mut ids_retried: Vec<Uuid> = Vec::new();
    let mut not_found = 0usize;
    let mut failed    = 0usize;

    for &id in &body.ids {
        let row = sqlx::query(
            "SELECT id, kind, payload FROM notification_dlq WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(pool.as_ref())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

        let Some(row) = row else { not_found += 1; continue; };

        let payload: serde_json::Value = row.get("payload");
        let notif: Notification = match serde_json::from_value(payload.clone()) {
            Ok(n)  => n,
            Err(_) => { failed += 1; continue; }
        };

        let _ = st.tx.send(notif.clone());
        NOTIFICATIONS_DISPATCHED.with_label_values(&[&notif.kind]).inc();

        if let Some(redis_pool) = &st.redis_pub {
            if let Ok(body_str) = serde_json::to_string(&notif) {
                if let Ok(mut conn) = redis_pool.get().await {
                    use deadpool_redis::redis::AsyncCommands;
                    let _ = conn.publish::<_, _, ()>("expresso:notifications", &body_str).await;
                }
            }
        }

        if let Some((url, client)) = &st.webhook {
            let url    = url.clone();
            let client = client.clone();
            let body_v = payload.clone();
            tokio::spawn(async move {
                let _ = client.post(url.as_ref()).json(&body_v).send().await;
            });
        }

        match sqlx::query("DELETE FROM notification_dlq WHERE id = $1")
            .bind(id)
            .execute(pool.as_ref())
            .await
        {
            Ok(_)  => ids_retried.push(id),
            Err(_) => failed += 1,
        }
    }

    Ok(Json(json!({
        "retried":     ids_retried.len(),
        "failed":      failed,
        "not_found":   not_found,
        "ids_retried": ids_retried,
    })))
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"service": SERVICE, "status": "ok"}))
}

async fn ready() -> Json<serde_json::Value> {
    Json(json!({"ready": true}))
}

// ─── Redis pub/sub (optional) ─────────────────────────────────────────────────

/// Build a Redis connection pool for publishing cross-pod notifications.
/// Returns None when REDIS_URL is unset.
async fn maybe_build_redis_pub() -> Option<Arc<deadpool_redis::Pool>> {
    let url = env::var("REDIS_URL").ok().filter(|u| !u.is_empty())?;
    let cfg = deadpool_redis::Config::from_url(url);
    match cfg.create_pool(Some(deadpool_redis::Runtime::Tokio1)) {
        Ok(pool) => {
            info!("Redis publish pool ready for cross-pod notifications");
            Some(Arc::new(pool))
        }
        Err(e) => {
            warn!(error = %e, "Redis publish pool init failed — cross-pod publish disabled");
            None
        }
    }
}

/// If REDIS_URL is set, spawn a background task that subscribes to
/// "expresso:notifications" and forwards messages into the in-process channel.
async fn maybe_start_redis_relay(tx: Arc<broadcast::Sender<Notification>>) {
    let url = match env::var("REDIS_URL") {
        Ok(u) if !u.is_empty() => u,
        _ => return,
    };

    info!(url = %url, "starting Redis relay for notifications");

    tokio::spawn(async move {
        loop {
            if let Err(e) = redis_relay_loop(&url, tx.clone()).await {
                warn!(error = %e, "Redis relay error; retrying in 5s");
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

async fn redis_relay_loop(
    url: &str,
    tx:  Arc<broadcast::Sender<Notification>>,
) -> anyhow::Result<()> {
    use deadpool_redis::redis::Client;
    let client = Client::open(url)?;
    let mut pubsub = client.get_async_pubsub().await?;
    pubsub.subscribe("expresso:notifications").await?;
    let mut stream = pubsub.into_on_message();
    while let Some(msg) = futures::StreamExt::next(&mut stream).await {
        let payload: String = msg.get_payload()?;
        if let Ok(notif) = serde_json::from_str::<Notification>(&payload) {
            let _ = tx.send(notif);
        }
    }
    Ok(())
}

// ─── OIDC validator (optional) ───────────────────────────────────────────────

async fn maybe_build_validator() -> Option<Arc<OidcValidator>> {
    let issuer   = env::var("AUTH__OIDC_ISSUER").ok().filter(|v| !v.is_empty())?;
    let audience = env::var("AUTH__OIDC_AUDIENCE").ok().filter(|v| !v.is_empty())?;

    let cfg = OidcConfig::new(issuer.clone(), audience);
    match OidcValidator::new(cfg).await {
        Ok(v) => {
            info!(issuer = %issuer, "OIDC validator ready — stream auth enabled");
            Some(Arc::new(v))
        }
        Err(e) => {
            warn!(error = %e, "OIDC validator init failed — stream auth DISABLED (dev mode)");
            None
        }
    }
}

// ─── Entrypoint ───────────────────────────────────────────────────────────────

async fn maybe_build_db() -> Option<Arc<DbPool>> {
    let url = env::var("DATABASE__URL").ok().filter(|v| !v.is_empty())?;
    let cfg = expresso_core::config::DatabaseConfig {
        url,
        max_connections: 5,
        min_connections: 1,
        acquire_timeout_secs: 5,
    };
    match create_db_pool(&cfg).await {
        Ok(pool) => {
            info!("database pool ready for notification persistence");
            Some(Arc::new(pool))
        }
        Err(e) => {
            warn!(error = %e, "database unavailable — digest endpoint disabled");
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
    format!("{host}:{port}").parse::<SocketAddr>()
        .map_err(|e| anyhow::anyhow!("invalid bind address: {}", e))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let (tx, _) = broadcast::channel::<Notification>(CHANNEL_CAP);
    let tx = Arc::new(tx);

    maybe_start_redis_relay(tx.clone()).await;

    let validator  = maybe_build_validator().await;
    let redis_pub  = maybe_build_redis_pub().await;
    let webhook    = env::var("NOTIFICATIONS__WEBHOOK_URL").ok()
        .filter(|v| !v.is_empty())
        .map(|url| (Arc::<str>::from(url.as_str()), reqwest::Client::new()));
    let db = maybe_build_db().await;
    let state = AppState { tx, validator, redis_pub, webhook, db };

    let app = Router::new()
        .route("/health",                          get(health))
        .route("/ready",                           get(ready))
        .route("/internal/notify",                 post(internal_notify))
        .route("/notifications/stream",            get(notifications_stream))
        .route("/api/v1/notifications/digest",     get(digest))
        .route("/api/v1/notifications/:id/read",   patch(mark_read))
        .route("/api/v1/notifications/read-all",   patch(mark_all_read))
        .route("/api/v1/notifications/push",       post(push_subscribe).delete(push_unsubscribe))
        .route("/api/v1/notifications/dlq/stats",            get(dlq_stats))
        .route("/api/v1/notifications/dlq/stats/by-day",          get(dlq_stats_by_day))
        .route("/api/v1/notifications/dlq/stats/by-hour",         get(dlq_stats_by_hour))
        .route("/api/v1/notifications/dlq/stats/by-kind-and-day",   get(dlq_stats_by_kind_and_day))
        .route("/api/v1/notifications/dlq/stats/by-tenant-and-day", get(dlq_stats_by_tenant_and_day))
        .route("/api/v1/notifications/dlq/stats/by-error-kind",     get(dlq_stats_by_error_kind))
        .route("/api/v1/notifications/dlq/stats/by-tenant",         get(dlq_stats_by_tenant))
        .route("/api/v1/notifications/dlq/stats/by-kind",             get(dlq_stats_by_kind))
        .route("/api/v1/notifications/dlq/stats/by-tenant-and-kind",  get(dlq_stats_by_tenant_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-user",                  get(dlq_stats_by_user))
        .route("/api/v1/notifications/dlq/stats/by-kind-and-user",          get(dlq_stats_by_kind_and_user))
        .route("/api/v1/notifications/dlq/stats/by-day-and-user",           get(dlq_stats_by_day_and_user))
        .route("/api/v1/notifications/dlq/stats/attempts-distribution", get(dlq_stats_attempts_distribution))
        .route("/api/v1/notifications/dlq/stats/retention",             get(dlq_stats_retention))
        .route("/api/v1/notifications/dlq/stats/by-day-and-kind",        get(dlq_stats_by_day_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-kind",       get(dlq_stats_by_hour_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-minute",              get(dlq_stats_by_minute))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-kind",     get(dlq_stats_by_minute_and_kind))
        .route("/api/v1/notifications/dlq/count",            get(count_dlq))
        .route("/api/v1/notifications/dlq/oldest",           get(oldest_dlq_entry))
        .route("/api/v1/notifications/dlq/newest",           get(newest_dlq_entry))
        .route("/api/v1/notifications/dlq/retry-all",        post(retry_all_dlq))
        .route("/api/v1/notifications/dlq/retry-filtered",   post(retry_filtered_dlq))
        .route("/api/v1/notifications/dlq/bulk",             patch(bulk_patch_dlq))
        .route("/api/v1/notifications/dlq/bulk/count",       post(bulk_count_dlq))
        .route("/api/v1/notifications/dlq/bulk/retry",       post(bulk_retry_dlq))
        .route("/api/v1/notifications/dlq",           get(list_dlq).delete(purge_dlq))
        .route("/api/v1/notifications/dlq/:id",       get(get_dlq_entry).delete(delete_dlq_entry).patch(patch_dlq_entry))
        .route("/api/v1/notifications/dlq/:id/retry", post(retry_dlq_entry))
        .merge(expresso_observability::metrics_router())
        .layer(middleware::from_fn_with_state(state.clone(), inject_validator))
        .with_state(state);

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(service = SERVICE, %addr, "listening");

    axum::serve(listener, app).await?;

    Ok(())
}

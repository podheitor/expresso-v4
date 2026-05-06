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

/// GET /api/v1/notifications/dlq/stats/by-kind-and-tenant?limit=N — DLQ COUNT GROUP BY (kind, tenant_id).
///
/// Análogo a by-tenant-and-kind (#710) com ordem (kind, tenant_id). `limit` default 50 max 500.
/// Retorna `{rows:[{kind,tenant_id,count}]}` ordenado por count DESC. Sprint #735.
async fn dlq_stats_by_kind_and_tenant(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let raw_limit = q.limit.unwrap_or(50).clamp(1, 500) as i64;

    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT kind, tenant_id::TEXT, COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY kind, tenant_id \
          ORDER BY count DESC \
          LIMIT $1",
    )
    .bind(raw_limit)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, tenant_id, count)| json!({"kind": kind, "tenant_id": tenant_id, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-tenant-and-hour?since=&until= — DLQ COUNT GROUP BY (tenant_id, hour).
///
/// DATE_TRUNC('hour') no eixo temporal, agrupa por tenant_id. Granularidade intra-dia cross-tenant.
/// Retorna `{rows:[{hour,tenant_id,count}]}` ordenado por hour ASC, tenant_id ASC. Sprint #740.
async fn dlq_stats_by_tenant_and_hour(
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
            tenant_id::TEXT, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour, tenant_id \
         ORDER BY hour ASC, tenant_id ASC",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(hour, tenant_id, count)| json!({"hour": hour, "tenant_id": tenant_id, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-kind-and-hour?since=&until= — DLQ COUNT GROUP BY (kind, hour).
///
/// Complementa by-hour-and-kind (#720) com ordem invertida (kind primeiro).
/// Retorna `{rows:[{kind,hour,count}]}` ordenado por kind ASC, hour ASC. Sprint #745.
async fn dlq_stats_by_kind_and_hour(
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
            kind, \
            to_char(date_trunc('hour', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:00:00\"Z\"') AS hour, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY kind, hour \
         ORDER BY kind ASC, hour ASC",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, hour, count)| json!({"kind": kind, "hour": hour, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-error-prefix?limit=N — top-N prefixos de last_error.
///
/// Trunca `last_error` nos primeiros 60 chars e agrupa para revelar classes de erro repetidas.
/// `limit` default 20 max 200. Retorna `{rows:[{error_prefix,count}]}` count DESC. Sprint #750.
async fn dlq_stats_by_error_prefix(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let limit = q.limit.unwrap_or(20).clamp(1, 200) as i64;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT LEFT(last_error, 60) AS error_prefix, COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          WHERE last_error IS NOT NULL AND last_error <> '' \
          GROUP BY error_prefix \
          ORDER BY count DESC \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(error_prefix, count)| json!({"error_prefix": error_prefix, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/summary — rollup global DLQ: total + contagem por kind.
///
/// Snapshot de saúde da DLQ sem filtro temporal. Retorna `{total,by_kind:[{kind,count}]}`. Sprint #755.
async fn dlq_stats_summary(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let (total,): (i64,) = sqlx::query_as("SELECT COUNT(*)::BIGINT FROM notification_dlq")
        .fetch_one(pool.as_ref()).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let by_kind: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, COUNT(*)::BIGINT FROM notification_dlq GROUP BY kind ORDER BY count DESC",
    )
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let by_kind_out: Vec<serde_json::Value> = by_kind.into_iter()
        .map(|(kind, count)| json!({"kind": kind, "count": count}))
        .collect();
    Ok(Json(json!({"total": total, "by_kind": by_kind_out})))
}

/// GET /api/v1/notifications/dlq/stats/by-tenant-and-kind-and-day?since=&until= — 3D GROUP BY.
///
/// GROUP BY (tenant_id, kind, day) ASC — visão completa para dashboards de operação multi-tenant.
/// Retorna `{rows:[{day,tenant_id,kind,count}]}`. Sprint #760.
async fn dlq_stats_by_tenant_and_kind_and_day(
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

    let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            tenant_id::TEXT, kind, COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, tenant_id, kind \
         ORDER BY day ASC, tenant_id ASC, kind ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, tenant_id, kind, count)| json!({"day": day, "tenant_id": tenant_id, "kind": kind, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-tenant?since=&until= — GROUP BY (day, tenant_id).
///
/// Simetria com by-tenant-and-kind-and-day mas sem kind. Retorna `{rows:[{day,tenant_id,count}]}` day ASC. Sprint #765.
async fn dlq_stats_by_day_and_tenant(
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
            tenant_id::TEXT, COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, tenant_id \
         ORDER BY day ASC, tenant_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, tenant_id, count)| json!({"day": day, "tenant_id": tenant_id, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/age-distribution — histograma de idade das entradas DLQ.
///
/// Buckets: <1h / 1-6h / 6-24h / 1-7d / >7d por (NOW() - failed_at).
/// Retorna `{lt_1h,h1_to_6h,h6_to_24h,d1_to_7d,gt_7d}`. Sprint #770.
async fn dlq_stats_age_distribution(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let (lt_1h, h1_to_6h, h6_to_24h, d1_to_7d, gt_7d): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT \
                COUNT(*) FILTER (WHERE failed_at >= NOW() - INTERVAL '1 hour')::BIGINT, \
                COUNT(*) FILTER (WHERE failed_at >= NOW() - INTERVAL '6 hours'  AND failed_at < NOW() - INTERVAL '1 hour')::BIGINT, \
                COUNT(*) FILTER (WHERE failed_at >= NOW() - INTERVAL '24 hours' AND failed_at < NOW() - INTERVAL '6 hours')::BIGINT, \
                COUNT(*) FILTER (WHERE failed_at >= NOW() - INTERVAL '7 days'   AND failed_at < NOW() - INTERVAL '24 hours')::BIGINT, \
                COUNT(*) FILTER (WHERE failed_at <  NOW() - INTERVAL '7 days')::BIGINT \
             FROM notification_dlq",
        )
        .fetch_one(pool.as_ref()).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({
        "lt_1h":     lt_1h,
        "h1_to_6h":  h1_to_6h,
        "h6_to_24h": h6_to_24h,
        "d1_to_7d":  d1_to_7d,
        "gt_7d":     gt_7d,
    })))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-tenant?since=&until= — GROUP BY (hour, tenant_id).
///
/// Simetria com by-tenant-and-hour (#740) mas ordena por hour ASC, tenant_id ASC.
/// Retorna `{rows:[{hour,tenant_id,count}]}`. Sprint #775.
async fn dlq_stats_by_hour_and_tenant(
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

    let rows: Vec<(String, Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('hour', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:00:00\"Z\"') AS hour, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour, tenant_id \
         ORDER BY hour ASC, tenant_id ASC",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(hour, tenant_id, count)| json!({"hour": hour, "tenant_id": tenant_id, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-user-and-kind?limit=N — GROUP BY (user_id, kind).
///
/// Complementa by-kind-and-user (#705) com chave primária em user_id.
/// Limit default 50 max 500. Retorna `{rows:[{user_id,kind,count}]}` count DESC. Sprint #780.
async fn dlq_stats_by_user_and_kind(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let limit = q.limit.unwrap_or(50).min(500).max(1);

    let rows: Vec<(Option<Uuid>, String, i64)> = sqlx::query_as(
        "SELECT user_id, kind, COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY user_id, kind \
          ORDER BY count DESC \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(uid, kind, count)| json!({"user_id": uid, "kind": kind, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-kind-and-tenant — 3D GROUP BY (day, kind, tenant_id).
///
/// Análogo a by-tenant-and-kind-and-day (#760) mas ordenado por day ASC, kind ASC, tenant_id ASC.
/// Retorna `{rows:[{day,kind,tenant_id,count}]}`. Sprint #785.
async fn dlq_stats_by_day_and_kind_and_tenant(
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

    let rows: Vec<(String, String, Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            kind, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, kind, tenant_id \
         ORDER BY day ASC, kind ASC, tenant_id ASC",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, kind, tid, count)| json!({"day": day, "kind": kind, "tenant_id": tid, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-user?since=&until= — GROUP BY (hour, user_id).
///
/// Granularidade intra-dia por usuário. Análogo a by-hour-and-tenant (#775) mas por user_id.
/// Retorna `{rows:[{hour,user_id,count}]}` ordenado por hour ASC, user_id ASC. Sprint #790.
async fn dlq_stats_by_hour_and_user(
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

    let rows: Vec<(String, Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('hour', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:00:00\"Z\"') AS hour, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour, user_id \
         ORDER BY hour ASC, user_id ASC",
    )
    .bind(since_dt)
    .bind(until_dt)
    .fetch_all(pool.as_ref())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(hour, uid, count)| json!({"hour": hour, "user_id": uid, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-tenant?since=&until= — GROUP BY (minute, tenant_id).
///
/// Granularidade fina cross-tenant. Retorna `{rows:[{minute,tenant_id,count}]}` minute ASC. Sprint #795.
async fn dlq_stats_by_minute_and_tenant(
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

    let rows: Vec<(String, Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('minute', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:00\"Z\"') AS minute, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute, tenant_id \
         ORDER BY minute ASC, tenant_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, t, c)| json!({"minute": m, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-user?since=&until= — GROUP BY (minute, user_id).
///
/// Granularidade fina por usuário. Retorna `{rows:[{minute,user_id,count}]}` minute ASC. Sprint #800.
async fn dlq_stats_by_minute_and_user(
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

    let rows: Vec<(String, Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('minute', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:00\"Z\"') AS minute, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute, user_id \
         ORDER BY minute ASC, user_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, u, c)| json!({"minute": m, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/top-tenants-by-kind?limit=N — top-N tenants por kind.
///
/// Para cada kind distinto, lista os `limit` tenants com mais entradas.
/// Limit default 5 max 50. Retorna `{rows:[{kind,tenant_id,count}]}` kind ASC, count DESC. Sprint #805.
async fn dlq_stats_top_tenants_by_kind(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let limit = q.limit.unwrap_or(5).min(50).max(1);

    let rows: Vec<(String, Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT kind, tenant_id, COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY kind, tenant_id \
          ORDER BY kind ASC, count DESC \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(k, t, c)| json!({"kind": k, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-attempts-and-kind?since=&until= — histograma tentativas × kind.
///
/// Para cada bucket de attempts (1/2/3/4/5+), quebra por kind.
/// Retorna `{rows:[{attempts_bucket,kind,count}]}` bucket ASC, kind ASC. Sprint #815.
async fn dlq_stats_by_attempts_and_kind(
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
            CASE \
                WHEN attempts = 1 THEN '1' \
                WHEN attempts = 2 THEN '2' \
                WHEN attempts = 3 THEN '3' \
                WHEN attempts = 4 THEN '4' \
                ELSE '5+' \
            END AS attempts_bucket, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY attempts_bucket, kind \
         ORDER BY attempts_bucket ASC, kind ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(b, k, c)| json!({"attempts_bucket": b, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-tenant-and-minute?since=&until= — GROUP BY (minute, tenant_id) ASC.
///
/// Granularidade de minuto cruzada com tenant. Retorna `{rows:[{minute,tenant_id,count}]}`. Sprint #820.
async fn dlq_stats_by_tenant_and_minute(
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
            to_char(date_trunc('minute', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:00\"Z\"') AS minute, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute, tenant_id \
         ORDER BY minute ASC, tenant_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, t, c)| json!({"minute": m, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-user-and-kind?since=&until= — 3D GROUP BY (day, user_id, kind).
///
/// Detalhamento máximo: dia × usuário × tipo. Retorna `{rows:[{day,user_id,kind,count}]}`. Sprint #825.
async fn dlq_stats_by_day_and_user_and_kind(
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

    let rows: Vec<(String, Option<uuid::Uuid>, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            user_id, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, user_id, kind \
         ORDER BY day ASC, user_id ASC, kind ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, u, k, c)| json!({"day": d, "user_id": u, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-kind-and-tenant?since=&until= — 3D GROUP BY (hour, kind, tenant_id).
///
/// Granularidade de hora cruzada com kind e tenant. Retorna `{rows:[{hour,kind,tenant_id,count}]}`. Sprint #830.
async fn dlq_stats_by_hour_and_kind_and_tenant(
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

    let rows: Vec<(String, String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('hour', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:00:00\"Z\"') AS hour, \
            kind, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour, kind, tenant_id \
         ORDER BY hour ASC, kind ASC, tenant_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, k, t, c)| json!({"hour": h, "kind": k, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-kind-and-minute?since=&until= — GROUP BY (kind, minute) ASC.
///
/// Complementa by-minute-and-kind (#730) com chave primária em kind.
/// Retorna `{rows:[{kind,minute,count}]}`. Sprint #810.
async fn dlq_stats_by_kind_and_minute(
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
            kind, \
            to_char(date_trunc('minute', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:00\"Z\"') AS minute, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY kind, minute \
         ORDER BY kind ASC, minute ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(k, m, c)| json!({"kind": k, "minute": m, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-kind-and-tenant?since=&until= — 3D GROUP BY (minute, kind, tenant_id).
///
/// Granularidade de minuto cruzada com kind e tenant_id. Retorna `{rows:[{minute,kind,tenant_id,count}]}`. Sprint #835.
async fn dlq_stats_by_minute_and_kind_and_tenant(
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

    let rows: Vec<(String, String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('minute', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:00\"Z\"') AS minute, \
            kind, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute, kind, tenant_id \
         ORDER BY minute ASC, kind ASC, tenant_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, k, t, c)| json!({"minute": m, "kind": k, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-kind?since=&until= — GROUP BY (second, kind) ASC.
///
/// Granularidade de segundo por tipo. Útil para burst analysis. Retorna `{rows:[{second,kind,count}]}`. Sprint #840.
async fn dlq_stats_by_second_and_kind(
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
            to_char(date_trunc('second', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS second, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second, kind \
         ORDER BY second ASC, kind ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, k, c)| json!({"second": s, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-user-and-kind?since=&until= — 3D GROUP BY (minute, user_id, kind).
///
/// Granularidade de minuto cruzada com user_id e kind. Retorna `{rows:[{minute,user_id,kind,count}]}`. Sprint #845.
async fn dlq_stats_by_minute_and_user_and_kind(
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

    let rows: Vec<(String, Option<uuid::Uuid>, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('minute', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:00\"Z\"') AS minute, \
            user_id, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute, user_id, kind \
         ORDER BY minute ASC, user_id ASC, kind ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, u, k, c)| json!({"minute": m, "user_id": u, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-tenant?since=&until= — GROUP BY (second, tenant_id) ASC.
///
/// Granularidade de segundo por tenant. Retorna `{rows:[{second,tenant_id,count}]}`. Sprint #850.
async fn dlq_stats_by_second_and_tenant(
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
            to_char(date_trunc('second', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS second, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second, tenant_id \
         ORDER BY second ASC, tenant_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, t, c)| json!({"second": s, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-user?since=&until= — GROUP BY (second, user_id) ASC.
///
/// Granularidade de segundo por usuário. Retorna `{rows:[{second,user_id,count}]}`. Sprint #855.
async fn dlq_stats_by_second_and_user(
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
            to_char(date_trunc('second', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS second, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second, user_id \
         ORDER BY second ASC, user_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, u, c)| json!({"second": s, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-kind-and-tenant?since=&until= — 3D GROUP BY (second, kind, tenant_id).
///
/// Granularidade de segundo cruzada com kind e tenant. Retorna `{rows:[{second,kind,tenant_id,count}]}`. Sprint #860.
async fn dlq_stats_by_second_and_kind_and_tenant(
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

    let rows: Vec<(String, String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('second', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS second, \
            kind, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second, kind, tenant_id \
         ORDER BY second ASC, kind ASC, tenant_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, k, t, c)| json!({"second": s, "kind": k, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-kind-and-user?since=&until= — 3D GROUP BY (minute, kind, user_id).
///
/// Granularidade de minuto cruzada com kind e user_id. Retorna `{rows:[{minute,kind,user_id,count}]}`. Sprint #865.
async fn dlq_stats_by_minute_and_kind_and_user(
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

    let rows: Vec<(String, String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('minute', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:00\"Z\"') AS minute, \
            kind, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute, kind, user_id \
         ORDER BY minute ASC, kind ASC, user_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, k, u, c)| json!({"minute": m, "kind": k, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-user-and-tenant?limit=N — GROUP BY (user_id, tenant_id) COUNT DESC.
///
/// Retorna `{rows:[{user_id,tenant_id,count}]}` count DESC. Sprint #895.
async fn dlq_stats_by_user_and_tenant(
    State(st): State<AppState>,
    Query(q):  Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let limit = q.limit.unwrap_or(50).min(500).max(1);

    let rows: Vec<(Option<uuid::Uuid>, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT user_id, tenant_id, COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY user_id, tenant_id \
          ORDER BY count DESC \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(u, t, c)| json!({"user_id": u, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-kind-and-day-and-tenant?since=&until= — 3D kind×day×tenant.
///
/// GROUP BY (kind, day, tenant_id) ASC. Retorna `{rows:[{kind,day,tenant_id,count}]}`. Sprint #900.
async fn dlq_stats_by_kind_and_day_and_tenant(
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

    let rows: Vec<(String, String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            kind, \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY kind, day, tenant_id \
         ORDER BY kind ASC, day ASC, tenant_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(k, d, t, c)| json!({"kind": k, "day": d, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/error-length-by-kind — avg/max LENGTH(last_error) por kind.
///
/// Identifica kinds com mensagens de erro mais longas. Retorna `{rows:[{kind,avg_length,max_length,with_error,count}]}`. Sprint #905.
async fn dlq_stats_error_length_by_kind(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let rows: Vec<(String, Option<f64>, Option<i64>, i64, i64)> = sqlx::query_as(
        "SELECT \
            kind, \
            AVG(LENGTH(last_error))::FLOAT8  AS avg_length, \
            MAX(LENGTH(last_error))::BIGINT  AS max_length, \
            COUNT(*) FILTER (WHERE last_error IS NOT NULL)::BIGINT AS with_error, \
            COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY kind \
          ORDER BY avg_length DESC NULLS LAST",
    )
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(k, avg, max, we, c)| json!({
            "kind": k, "avg_length": avg, "max_length": max,
            "with_error": we, "count": c
        }))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/tenant-coverage — COUNT DISTINCT tenant_id + user_id no DLQ.
///
/// Retorna `{distinct_tenants,distinct_users,total_entries}`. Sprint #910.
async fn dlq_stats_tenant_coverage(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let (distinct_tenants, distinct_users, total): (i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(DISTINCT tenant_id)::BIGINT AS distinct_tenants, \
            COUNT(DISTINCT user_id)::BIGINT   AS distinct_users, \
            COUNT(*)::BIGINT                  AS total_entries \
           FROM notification_dlq",
    )
    .fetch_one(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({
        "distinct_tenants": distinct_tenants,
        "distinct_users":   distinct_users,
        "total_entries":    total,
    })))
}

/// GET /api/v1/notifications/dlq/stats/user-coverage?limit=N — COUNT DISTINCT user_id por tenant_id.
///
/// GROUP BY tenant_id ORDER BY distinct_users DESC; default limit 50. Sprint #930.
async fn dlq_stats_user_coverage(
    State(st): State<AppState>,
    Query(q):  Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let limit = q.limit.unwrap_or(50).min(500).max(1);

    let rows: Vec<(Option<Uuid>, i64, i64)> = sqlx::query_as(
        "SELECT \
            tenant_id, \
            COUNT(DISTINCT user_id)::BIGINT AS distinct_users, \
            COUNT(*)::BIGINT               AS total_entries \
           FROM notification_dlq \
          GROUP BY tenant_id \
          ORDER BY distinct_users DESC \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(tenant, users, total)| json!({"tenant_id": tenant, "distinct_users": users, "total_entries": total}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-day-and-kind?since=&until= — 3D (day, hour, kind) COUNT. Sprint #1220.
async fn dlq_stats_by_hour_and_day_and_kind(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since = q.since.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;
    let until = q.until.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;

    let rows: Vec<(String, i32, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(DATE_TRUNC('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            kind, \
            COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
            AND ($2::timestamptz IS NULL OR failed_at <  $2) \
          GROUP BY day, hour_of_day, kind \
          ORDER BY day ASC, hour_of_day ASC, kind ASC",
    )
    .bind(since).bind(until)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, hour, kind, count)| json!({"day": day, "hour": hour, "kind": kind, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-day-and-user?since=&until= — 3D (day, hour, user_id) COUNT. Sprint #1215.
async fn dlq_stats_by_hour_and_day_and_user(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since = q.since.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;
    let until = q.until.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;

    let rows: Vec<(String, i32, Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(DATE_TRUNC('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            user_id, \
            COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
            AND ($2::timestamptz IS NULL OR failed_at <  $2) \
          GROUP BY day, hour_of_day, user_id \
          ORDER BY day ASC, hour_of_day ASC, user_id ASC",
    )
    .bind(since).bind(until)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, hour, user, count)| json!({"day": day, "hour": hour, "user_id": user, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-day-and-tenant?since=&until= — 3D (day, hour, tenant_id).
///
/// GROUP BY (day, hour_of_day, tenant_id) ASC. Sprint #925.
async fn dlq_stats_by_hour_and_day_and_tenant(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since = q.since.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;
    let until = q.until.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;

    let rows: Vec<(String, i32, Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(DATE_TRUNC('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
            AND ($2::timestamptz IS NULL OR failed_at <  $2) \
          GROUP BY day, hour_of_day, tenant_id \
          ORDER BY day ASC, hour_of_day ASC, tenant_id ASC",
    )
    .bind(since).bind(until)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, hour, tenant, count)| json!({"day": day, "hour": hour, "tenant_id": tenant, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-hour-and-kind?since=&until= — 3D (day, hour, kind).
///
/// GROUP BY (day, hour_of_day, kind) ASC. Sprint #920.
async fn dlq_stats_by_day_and_hour_and_kind(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since = q.since.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;
    let until = q.until.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;

    let rows: Vec<(String, i32, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(DATE_TRUNC('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COALESCE(kind, 'unknown') AS kind, \
            COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
            AND ($2::timestamptz IS NULL OR failed_at <  $2) \
          GROUP BY day, hour_of_day, kind \
          ORDER BY day ASC, hour_of_day ASC, kind ASC",
    )
    .bind(since).bind(until)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, hour, kind, count)| json!({"day": day, "hour": hour, "kind": kind, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-tenant-and-user-and-kind?limit=N — 3D tenant×user×kind.
///
/// GROUP BY (tenant_id, user_id, kind) COUNT DESC; default limit 50. Sprint #950.
async fn dlq_stats_by_tenant_and_user_and_kind(
    State(st): State<AppState>,
    Query(q):  Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let limit = q.limit.unwrap_or(50).min(500).max(1);

    let rows: Vec<(Option<Uuid>, Option<Uuid>, String, i64)> = sqlx::query_as(
        "SELECT \
            tenant_id, \
            user_id, \
            COALESCE(kind, 'unknown') AS kind, \
            COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY tenant_id, user_id, kind \
          ORDER BY count DESC \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(tenant, user, kind, count)| json!({"tenant_id": tenant, "user_id": user, "kind": kind, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-user-and-hour?since=&until= — GROUP BY (user_id, hour_of_day) COUNT.
///
/// ORDER BY user_id ASC, hour_of_day ASC. Sprint #945.
async fn dlq_stats_by_user_and_hour(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since = q.since.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;
    let until = q.until.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;

    let rows: Vec<(Option<Uuid>, i32, i64)> = sqlx::query_as(
        "SELECT \
            user_id, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
            AND ($2::timestamptz IS NULL OR failed_at <  $2) \
          GROUP BY user_id, hour_of_day \
          ORDER BY user_id ASC, hour_of_day ASC",
    )
    .bind(since).bind(until)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(user, hour, count)| json!({"user_id": user, "hour": hour, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-tenant-and-day-and-kind?since=&until= — 3D tenant×day×kind.
///
/// GROUP BY (tenant_id, day, kind) ORDER BY day ASC, tenant_id ASC. Sprint #940.
async fn dlq_stats_by_tenant_and_day_and_kind(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since = q.since.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;
    let until = q.until.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;

    let rows: Vec<(Option<Uuid>, String, String, i64)> = sqlx::query_as(
        "SELECT \
            tenant_id, \
            to_char(DATE_TRUNC('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            COALESCE(kind, 'unknown') AS kind, \
            COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
            AND ($2::timestamptz IS NULL OR failed_at <  $2) \
          GROUP BY tenant_id, day, kind \
          ORDER BY day ASC, tenant_id ASC, kind ASC",
    )
    .bind(since).bind(until)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(tenant, day, kind, count)| json!({"tenant_id": tenant, "day": day, "kind": kind, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-kind-and-user-and-day?since=&until= — 3D kind×user×day.
///
/// GROUP BY (kind, user_id, day) ORDER BY day ASC, kind ASC. Sprint #935.
async fn dlq_stats_by_kind_and_user_and_day(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since = q.since.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;
    let until = q.until.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;

    let rows: Vec<(String, Option<Uuid>, String, i64)> = sqlx::query_as(
        "SELECT \
            COALESCE(kind, 'unknown') AS kind, \
            user_id, \
            to_char(DATE_TRUNC('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
            AND ($2::timestamptz IS NULL OR failed_at <  $2) \
          GROUP BY kind, user_id, day \
          ORDER BY day ASC, kind ASC, user_id ASC",
    )
    .bind(since).bind(until)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(kind, user, day, count)| json!({"kind": kind, "user_id": user, "day": day, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-day?since=&until= — 2D (day, hour) granularidade.
///
/// DATE_TRUNC('hour') → extrai dia e hora; GROUP BY (day, hour) ASC. Sprint #915.
async fn dlq_stats_by_hour_and_day(
    State(st): State<AppState>,
    Query(q):  Query<DlqStatsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let since = q.since.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;
    let until = q.until.as_deref()
        .map(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))?;

    let rows: Vec<(String, i32, i64)> = sqlx::query_as(
        "SELECT \
            to_char(DATE_TRUNC('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
            AND ($2::timestamptz IS NULL OR failed_at <  $2) \
          GROUP BY day, hour_of_day \
          ORDER BY day ASC, hour_of_day ASC",
    )
    .bind(since).bind(until)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(day, hour, count)| json!({"day": day, "hour": hour, "count": count}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-attempts-and-tenant?limit=N — histograma attempts × tenant.
///
/// GROUP BY (attempts, tenant_id) COUNT DESC; default limit 50. Sprint #890.
async fn dlq_stats_by_attempts_and_tenant(
    State(st): State<AppState>,
    Query(q):  Query<StatsLimitQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;
    let limit = q.limit.unwrap_or(50).min(500).max(1);

    let rows: Vec<(i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT attempts, tenant_id, COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY attempts, tenant_id \
          ORDER BY count DESC \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(a, t, c)| json!({"attempts": a, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/failed-at-hour-distribution — histograma de hora do dia (0-23).
///
/// COUNT(*) GROUP BY EXTRACT(HOUR FROM failed_at). Retorna `{rows:[{hour_of_day,count}]}` hour ASC. Sprint #885.
async fn dlq_stats_failed_at_hour_distribution(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
                COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY hour_of_day \
          ORDER BY hour_of_day ASC",
    )
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, c)| json!({"hour_of_day": h, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/retry-rate-by-kind — AVG/MAX attempts por kind.
///
/// GROUP BY kind; retorna `{rows:[{kind,avg_attempts,max_attempts,count}]}` avg DESC. Sprint #880.
async fn dlq_stats_retry_rate_by_kind(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let pool = st.db.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "unavailable"})),
    ))?;

    let rows: Vec<(String, f64, i64, i64)> = sqlx::query_as(
        "SELECT kind, AVG(attempts)::FLOAT8 AS avg_attempts, MAX(attempts)::BIGINT AS max_attempts, COUNT(*)::BIGINT AS count \
           FROM notification_dlq \
          GROUP BY kind \
          ORDER BY avg_attempts DESC",
    )
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(k, avg, max, c)| json!({"kind": k, "avg_attempts": avg, "max_attempts": max, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-user-and-kind?since=&until= — 3D GROUP BY (hour, user_id, kind).
///
/// Granularidade de hora cruzada com user_id e kind. Retorna `{rows:[{hour,user_id,kind,count}]}`. Sprint #875.
async fn dlq_stats_by_hour_and_user_and_kind(
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

    let rows: Vec<(String, Option<uuid::Uuid>, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('hour', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:00:00\"Z\"') AS hour, \
            user_id, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour, user_id, kind \
         ORDER BY hour ASC, user_id ASC, kind ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, u, k, c)| json!({"hour": h, "user_id": u, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-kind-and-user?since=&until= — 3D GROUP BY (hour, kind, user_id).
///
/// Granularidade de hora cruzada com kind e user_id. Retorna `{rows:[{hour,kind,user_id,count}]}`. Sprint #870.
async fn dlq_stats_by_hour_and_kind_and_user(
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

    let rows: Vec<(String, String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('hour', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:00:00\"Z\"') AS hour, \
            kind, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour, kind, user_id \
         ORDER BY hour ASC, kind ASC, user_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, k, u, c)| json!({"hour": h, "kind": k, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-kind-and-tenant-and-hour — 3D kind×tenant×hour.
///
/// GROUP BY (kind, tenant_id, hour_of_day) ORDER BY hour ASC, kind ASC, tenant_id ASC.
/// Aceita `since`/`until` RFC3339. Sprint #955.
async fn dlq_stats_by_kind_and_tenant_and_hour(
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

    let rows: Vec<(String, Option<uuid::Uuid>, i32, i64)> = sqlx::query_as(
        "SELECT \
            kind, \
            tenant_id, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY kind, tenant_id, hour_of_day \
         ORDER BY hour_of_day ASC, kind ASC, tenant_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(k, t, h, c)| json!({"kind": k, "tenant_id": t, "hour_of_day": h, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-kind-and-hour-and-user — 3D kind×hour×user.
///
/// GROUP BY (kind, hour_of_day, user_id) ORDER BY hour ASC, kind ASC, user_id ASC.
/// Aceita `since`/`until` RFC3339. Sprint #960.
async fn dlq_stats_by_kind_and_hour_and_user(
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

    let rows: Vec<(String, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            kind, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY kind, hour_of_day, user_id \
         ORDER BY hour_of_day ASC, kind ASC, user_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(k, h, u, c)| json!({"kind": k, "hour_of_day": h, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-tenant-and-hour-and-user — 3D tenant×hour×user.
///
/// GROUP BY (tenant_id, hour_of_day, user_id) ORDER BY hour ASC, tenant_id ASC, user_id ASC.
/// Aceita `since`/`until` RFC3339. Sprint #965.
async fn dlq_stats_by_tenant_and_hour_and_user(
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

    let rows: Vec<(Option<uuid::Uuid>, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            tenant_id, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY tenant_id, hour_of_day, user_id \
         ORDER BY hour_of_day ASC, tenant_id ASC, user_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(t, h, u, c)| json!({"tenant_id": t, "hour_of_day": h, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-user-and-day-and-hour — 3D user×day×hour.
///
/// GROUP BY (user_id, day, hour_of_day) ORDER BY day ASC, user_id ASC, hour_of_day ASC.
/// Aceita `since`/`until` RFC3339. Sprint #970.
async fn dlq_stats_by_user_and_day_and_hour(
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

    let rows: Vec<(Option<uuid::Uuid>, String, i32, i64)> = sqlx::query_as(
        "SELECT \
            user_id, \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY user_id, day, hour_of_day \
         ORDER BY day ASC, user_id ASC, hour_of_day ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(u, d, h, c)| json!({"user_id": u, "day": d, "hour_of_day": h, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-kind-and-user-and-hour — 3D kind×user×hour.
///
/// GROUP BY (kind, user_id, hour_of_day) COUNT DESC. Aceita since/until RFC3339. Sprint #975.
async fn dlq_stats_by_kind_and_user_and_hour(
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

    let rows: Vec<(String, Option<uuid::Uuid>, i32, i64)> = sqlx::query_as(
        "SELECT \
            kind, \
            user_id, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY kind, user_id, hour_of_day \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(k, u, h, c)| json!({"kind": k, "user_id": u, "hour_of_day": h, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-kind-and-user — 3D day×kind×user.
///
/// GROUP BY (day, kind, user_id) ORDER BY day ASC. Aceita since/until RFC3339. Sprint #980.
async fn dlq_stats_by_day_and_kind_and_user(
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

    let rows: Vec<(String, String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            kind, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, kind, user_id \
         ORDER BY day ASC, kind ASC, user_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, k, u, c)| json!({"day": d, "kind": k, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-tenant-and-kind-and-minute — 3D tenant×kind×minute.
///
/// DATE_TRUNC('minute') × kind × tenant_id. Aceita since/until RFC3339. Sprint #985.
async fn dlq_stats_by_tenant_and_kind_and_minute(
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

    let rows: Vec<(Option<uuid::Uuid>, String, String, i64)> = sqlx::query_as(
        "SELECT \
            tenant_id, \
            kind, \
            to_char(date_trunc('minute', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:00\"Z\"') AS minute, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY tenant_id, kind, minute \
         ORDER BY minute ASC, tenant_id ASC, kind ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(t, k, m, c)| json!({"tenant_id": t, "kind": k, "minute": m, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-user-and-kind-and-minute — 3D user×kind×minute.
///
/// DATE_TRUNC('minute') × kind × user_id. Aceita since/until RFC3339. Sprint #990.
async fn dlq_stats_by_user_and_kind_and_minute(
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

    let rows: Vec<(Option<uuid::Uuid>, String, String, i64)> = sqlx::query_as(
        "SELECT \
            user_id, \
            kind, \
            to_char(date_trunc('minute', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:00\"Z\"') AS minute, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY user_id, kind, minute \
         ORDER BY minute ASC, user_id ASC, kind ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(u, k, m, c)| json!({"user_id": u, "kind": k, "minute": m, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-kind-and-day-and-hour — 3D kind×day×hour. Sprint #995.
async fn dlq_stats_by_kind_and_day_and_hour(
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

    let rows: Vec<(String, String, i32, i64)> = sqlx::query_as(
        "SELECT \
            kind, \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY kind, day, hour_of_day \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(k, d, h, c)| json!({"kind": k, "day": d, "hour_of_day": h, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-day — COUNT per (day, second) for micro-burst analysis. Sprint #996.
async fn dlq_stats_by_second_and_day(
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

    let rows: Vec<(String, i32, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, second_of_minute \
         ORDER BY day ASC, second_of_minute ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, s, c)| json!({"day": d, "second_of_minute": s, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-tenant-and-day — 3D minute×tenant×day. Sprint #997.
async fn dlq_stats_by_minute_and_tenant_and_day(
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

    let rows: Vec<(String, Option<uuid::Uuid>, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('minute', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:00\"Z\"') AS minute, \
            tenant_id, \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute, tenant_id, day \
         ORDER BY minute ASC, tenant_id ASC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, t, d, c)| json!({"minute": m, "tenant_id": t, "day": d, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-second-and-kind — 3D day×second×kind. Sprint #998.
async fn dlq_stats_by_day_and_second_and_kind(
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

    let rows: Vec<(String, i32, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, second_of_minute, kind \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, s, k, c)| json!({"day": d, "second_of_minute": s, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-day-and-tenant — 3D second×day×tenant COUNT. Sprint #1195.
async fn dlq_stats_by_second_and_day_and_tenant(
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

    let rows: Vec<(i32, String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second_of_minute, day, tenant_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, d, t, c)| json!({"second_of_minute": s, "day": d, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-day-and-kind — 3D minute×day×kind COUNT. Sprint #1200.
async fn dlq_stats_by_minute_and_day_and_kind(
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

    let rows: Vec<(i32, String, String, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute_of_hour, day, kind \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, d, k, c)| json!({"minute_of_hour": m, "day": d, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-day-and-user — 3D minute×day×user COUNT. Sprint #1205.
async fn dlq_stats_by_minute_and_day_and_user(
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

    let rows: Vec<(i32, String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute_of_hour, day, user_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, d, u, c)| json!({"minute_of_hour": m, "day": d, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-day-and-tenant — 3D minute×day×tenant COUNT. Sprint #1210.
async fn dlq_stats_by_minute_and_day_and_tenant(
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

    let rows: Vec<(i32, String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute_of_hour, day, tenant_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, d, t, c)| json!({"minute_of_hour": m, "day": d, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-day-and-kind — 3D second×day×kind COUNT. Sprint #1185.
async fn dlq_stats_by_second_and_day_and_kind(
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

    let rows: Vec<(i32, String, String, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second_of_minute, day, kind \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, d, k, c)| json!({"second_of_minute": s, "day": d, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-day-and-user — 3D second×day×user COUNT. Sprint #1190.
async fn dlq_stats_by_second_and_day_and_user(
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

    let rows: Vec<(i32, String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second_of_minute, day, user_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, d, u, c)| json!({"second_of_minute": s, "day": d, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-second-and-user — 3D day×second×user COUNT. Sprint #1175.
async fn dlq_stats_by_day_and_second_and_user(
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

    let rows: Vec<(String, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, second_of_minute, user_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, s, u, c)| json!({"day": d, "second_of_minute": s, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-second-and-tenant — 3D day×second×tenant COUNT. Sprint #1180.
async fn dlq_stats_by_day_and_second_and_tenant(
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

    let rows: Vec<(String, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, second_of_minute, tenant_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, s, t, c)| json!({"day": d, "second_of_minute": s, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-kind-and-tenant — 3D second×kind×tenant. Sprint #1015.
async fn dlq_stats_by_second_and_kind_and_tenant(
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

    let rows: Vec<(i32, String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            kind, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second_of_minute, kind, tenant_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, k, t, c)| json!({"second_of_minute": s, "kind": k, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-tenant-and-kind — 3D hour×tenant×kind. Sprint #1016.
async fn dlq_stats_by_hour_and_tenant_and_kind(
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

    let rows: Vec<(i32, Option<uuid::Uuid>, String, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            tenant_id, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour_of_day, tenant_id, kind \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, t, k, c)| json!({"hour_of_day": h, "tenant_id": t, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-kind-and-tenant — 3D minute×kind×tenant. Sprint #1017.
async fn dlq_stats_by_minute_and_kind_and_tenant(
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

    let rows: Vec<(String, String, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('minute', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD\"T\"HH24:MI:00\"Z\"') AS minute, \
            kind, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute, kind, tenant_id \
         ORDER BY minute ASC, count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, k, t, c)| json!({"minute": m, "kind": k, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-tenant-and-kind — 3D day×tenant×kind. Sprint #1018.
async fn dlq_stats_by_day_and_tenant_and_kind(
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

    let rows: Vec<(String, Option<uuid::Uuid>, String, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            tenant_id, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, tenant_id, kind \
         ORDER BY day ASC, count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, t, k, c)| json!({"day": d, "tenant_id": t, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-second — 2D hour×second sub-minuto. Sprint #1035.
async fn dlq_stats_by_hour_and_second(
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

    let rows: Vec<(i32, i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR   FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour_of_day, second_of_minute \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, s, c)| json!({"hour_of_day": h, "second_of_minute": s, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-tenant-and-second — COUNT per (tenant, second). Sprint #1036.
async fn dlq_stats_by_tenant_and_second(
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

    let rows: Vec<(Option<uuid::Uuid>, i32, i64)> = sqlx::query_as(
        "SELECT \
            tenant_id, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY tenant_id, second_of_minute \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(t, s, c)| json!({"tenant_id": t, "second_of_minute": s, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-kind-and-second — COUNT per (kind, second). Sprint #1037.
async fn dlq_stats_by_kind_and_second(
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

    let rows: Vec<(String, i32, i64)> = sqlx::query_as(
        "SELECT \
            kind, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY kind, second_of_minute \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(k, s, c)| json!({"kind": k, "second_of_minute": s, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-user-and-second — COUNT per (user_id, second). Sprint #1038.
async fn dlq_stats_by_user_and_second(
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

    let rows: Vec<(Option<uuid::Uuid>, i32, i64)> = sqlx::query_as(
        "SELECT \
            user_id, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY user_id, second_of_minute \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(u, s, c)| json!({"user_id": u, "second_of_minute": s, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-minute — COUNT per (second, minute) micro-burst matrix. Sprint #1055.
async fn dlq_stats_by_second_and_minute(
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

    let rows: Vec<(i32, i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second_of_minute, minute_of_hour \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, m, c)| json!({"second_of_minute": s, "minute_of_hour": m, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-minute-and-user — 3D day×minute×user COUNT. Sprint #1056.
async fn dlq_stats_by_day_and_minute_and_user(
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

    let rows: Vec<(String, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', failed_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day, minute_of_hour, user_id \
         ORDER BY day ASC, minute_of_hour ASC, count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, m, u, c)| json!({"day": d, "minute_of_hour": m, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-minute — 2D hour×minute intra-hora COUNT DESC. Sprint #1057.
async fn dlq_stats_by_hour_and_minute(
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

    let rows: Vec<(i32, i32, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR   FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour_of_day, minute_of_hour \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, m, c)| json!({"hour_of_day": h, "minute_of_hour": m, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-user-and-kind — 3D second×user×kind COUNT DESC. Sprint #1058.
async fn dlq_stats_by_second_and_user_and_kind(
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

    let rows: Vec<(i32, Option<uuid::Uuid>, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            user_id, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second_of_minute, user_id, kind \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, u, k, c)| json!({"second_of_minute": s, "user_id": u, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-second-and-kind — 3D minute×second×kind COUNT. Sprint #1075.
async fn dlq_stats_by_minute_and_second_and_kind(
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

    let rows: Vec<(i32, i32, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute_of_hour, second_of_minute, kind \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, s, k, c)| json!({"minute_of_hour": m, "second_of_minute": s, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-second-and-user — 3D minute×second×user COUNT. Sprint #1080.
async fn dlq_stats_by_minute_and_second_and_user(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute_of_hour, second_of_minute, user_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, s, u, c)| json!({"minute_of_hour": m, "second_of_minute": s, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-second-and-tenant — 3D minute×second×tenant COUNT. Sprint #1085.
async fn dlq_stats_by_minute_and_second_and_tenant(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute_of_hour, second_of_minute, tenant_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, s, t, c)| json!({"minute_of_hour": m, "second_of_minute": s, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-minute-and-tenant — 3D second×minute×tenant COUNT. Sprint #1090.
async fn dlq_stats_by_second_and_minute_and_tenant(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second_of_minute, minute_of_hour, tenant_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, m, t, c)| json!({"second_of_minute": s, "minute_of_hour": m, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-second-and-user — 3D hour×second×user COUNT. Sprint #1110.
async fn dlq_stats_by_hour_and_second_and_user(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR   FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour_of_day, second_of_minute, user_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, s, u, c)| json!({"hour_of_day": h, "second_of_minute": s, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-hour-and-kind — 3D minute×hour×kind COUNT. Sprint #1135.
async fn dlq_stats_by_minute_and_hour_and_kind(
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

    let rows: Vec<(i32, i32, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            EXTRACT(HOUR   FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute_of_hour, hour_of_day, kind \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, h, k, c)| json!({"minute_of_hour": m, "hour_of_day": h, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-hour-and-user — 3D minute×hour×user COUNT. Sprint #1140.
async fn dlq_stats_by_minute_and_hour_and_user(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            EXTRACT(HOUR   FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute_of_hour, hour_of_day, user_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, h, u, c)| json!({"minute_of_hour": m, "hour_of_day": h, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-minute-and-hour-and-tenant — 3D minute×hour×tenant COUNT. Sprint #1145.
async fn dlq_stats_by_minute_and_hour_and_tenant(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(MINUTE  FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            EXTRACT(HOUR    FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY minute_of_hour, hour_of_day, tenant_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(m, h, t, c)| json!({"minute_of_hour": m, "hour_of_day": h, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-hour-and-user — 3D second×hour×user COUNT. Sprint #1165.
async fn dlq_stats_by_second_and_hour_and_user(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            EXTRACT(HOUR   FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second_of_minute, hour_of_day, user_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, h, u, c)| json!({"second_of_minute": s, "hour_of_day": h, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-hour-and-tenant — 3D second×hour×tenant COUNT. Sprint #1170.
async fn dlq_stats_by_second_and_hour_and_tenant(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            EXTRACT(HOUR   FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second_of_minute, hour_of_day, tenant_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, h, t, c)| json!({"second_of_minute": s, "hour_of_day": h, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-hour-and-tenant — 3D day×hour×tenant COUNT. Sprint #1155.
async fn dlq_stats_by_day_and_hour_and_tenant(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW  FROM failed_at AT TIME ZONE 'UTC')::INT AS day_of_week, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day_of_week, hour_of_day, tenant_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, h, t, c)| json!({"day_of_week": d, "hour_of_day": h, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-hour-and-kind — 3D second×hour×kind COUNT. Sprint #1160.
async fn dlq_stats_by_second_and_hour_and_kind(
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

    let rows: Vec<(i32, i32, String, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            EXTRACT(HOUR   FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second_of_minute, hour_of_day, kind \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, h, k, c)| json!({"second_of_minute": s, "hour_of_day": h, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-day-and-hour-and-user — 3D day×hour×user COUNT. Sprint #1150.
async fn dlq_stats_by_day_and_hour_and_user(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(DOW  FROM failed_at AT TIME ZONE 'UTC')::INT AS day_of_week, \
            EXTRACT(HOUR FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY day_of_week, hour_of_day, user_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(d, h, u, c)| json!({"day_of_week": d, "hour_of_day": h, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-minute-and-user — 3D hour×minute×user COUNT. Sprint #1125.
async fn dlq_stats_by_hour_and_minute_and_user(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR   FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour_of_day, minute_of_hour, user_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, m, u, c)| json!({"hour_of_day": h, "minute_of_hour": m, "user_id": u, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-minute-and-tenant — 3D hour×minute×tenant COUNT. Sprint #1130.
async fn dlq_stats_by_hour_and_minute_and_tenant(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR   FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour_of_day, minute_of_hour, tenant_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, m, t, c)| json!({"hour_of_day": h, "minute_of_hour": m, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-second-and-tenant — 3D hour×second×tenant COUNT. Sprint #1115.
async fn dlq_stats_by_hour_and_second_and_tenant(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR   FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            tenant_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour_of_day, second_of_minute, tenant_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, s, t, c)| json!({"hour_of_day": h, "second_of_minute": s, "tenant_id": t, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-minute-and-kind — 3D hour×minute×kind COUNT. Sprint #1120.
async fn dlq_stats_by_hour_and_minute_and_kind(
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

    let rows: Vec<(i32, i32, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR   FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour_of_day, minute_of_hour, kind \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, m, k, c)| json!({"hour_of_day": h, "minute_of_hour": m, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-hour-and-second-and-kind — 3D hour×second×kind COUNT. Sprint #1105.
async fn dlq_stats_by_hour_and_second_and_kind(
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

    let rows: Vec<(i32, i32, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(HOUR   FROM failed_at AT TIME ZONE 'UTC')::INT AS hour_of_day, \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY hour_of_day, second_of_minute, kind \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(h, s, k, c)| json!({"hour_of_day": h, "second_of_minute": s, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-minute-and-kind — 3D second×minute×kind COUNT. Sprint #1100.
async fn dlq_stats_by_second_and_minute_and_kind(
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

    let rows: Vec<(i32, i32, Option<String>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            kind, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second_of_minute, minute_of_hour, kind \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, m, k, c)| json!({"second_of_minute": s, "minute_of_hour": m, "kind": k, "count": c}))
        .collect();
    Ok(Json(json!({"rows": result})))
}

/// GET /api/v1/notifications/dlq/stats/by-second-and-minute-and-user — 3D second×minute×user COUNT. Sprint #1095.
async fn dlq_stats_by_second_and_minute_and_user(
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

    let rows: Vec<(i32, i32, Option<uuid::Uuid>, i64)> = sqlx::query_as(
        "SELECT \
            EXTRACT(SECOND FROM failed_at AT TIME ZONE 'UTC')::INT AS second_of_minute, \
            EXTRACT(MINUTE FROM failed_at AT TIME ZONE 'UTC')::INT AS minute_of_hour, \
            user_id, \
            COUNT(*)::BIGINT AS count \
         FROM notification_dlq \
         WHERE ($1::timestamptz IS NULL OR failed_at >= $1) \
           AND ($2::timestamptz IS NULL OR failed_at <  $2) \
         GROUP BY second_of_minute, minute_of_hour, user_id \
         ORDER BY count DESC",
    )
    .bind(since_dt).bind(until_dt)
    .fetch_all(pool.as_ref()).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(s, m, u, c)| json!({"second_of_minute": s, "minute_of_hour": m, "user_id": u, "count": c}))
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
        .route("/api/v1/notifications/dlq/stats/by-kind-and-tenant",     get(dlq_stats_by_kind_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-tenant-and-hour",     get(dlq_stats_by_tenant_and_hour))
        .route("/api/v1/notifications/dlq/stats/by-kind-and-hour",       get(dlq_stats_by_kind_and_hour))
        .route("/api/v1/notifications/dlq/stats/by-error-prefix",        get(dlq_stats_by_error_prefix))
        .route("/api/v1/notifications/dlq/stats/summary",                get(dlq_stats_summary))
        .route("/api/v1/notifications/dlq/stats/by-tenant-and-kind-and-day", get(dlq_stats_by_tenant_and_kind_and_day))
        .route("/api/v1/notifications/dlq/stats/by-day-and-tenant",      get(dlq_stats_by_day_and_tenant))
        .route("/api/v1/notifications/dlq/stats/age-distribution",        get(dlq_stats_age_distribution))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-tenant",       get(dlq_stats_by_hour_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-user-and-kind",         get(dlq_stats_by_user_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-day-and-kind-and-tenant", get(dlq_stats_by_day_and_kind_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-user",         get(dlq_stats_by_hour_and_user))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-tenant",    get(dlq_stats_by_minute_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-user",      get(dlq_stats_by_minute_and_user))
        .route("/api/v1/notifications/dlq/stats/top-tenants-by-kind",     get(dlq_stats_top_tenants_by_kind))
        .route("/api/v1/notifications/dlq/stats/by-attempts-and-kind",       get(dlq_stats_by_attempts_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-tenant-and-minute",      get(dlq_stats_by_tenant_and_minute))
        .route("/api/v1/notifications/dlq/stats/by-day-and-user-and-kind",  get(dlq_stats_by_day_and_user_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-kind-and-tenant", get(dlq_stats_by_hour_and_kind_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-kind-and-minute",      get(dlq_stats_by_kind_and_minute))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-kind-and-tenant", get(dlq_stats_by_minute_and_kind_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-second-and-kind",      get(dlq_stats_by_second_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-user-and-kind", get(dlq_stats_by_minute_and_user_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-second-and-tenant",   get(dlq_stats_by_second_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-second-and-user",    get(dlq_stats_by_second_and_user))
        .route("/api/v1/notifications/dlq/stats/by-second-and-kind-and-tenant", get(dlq_stats_by_second_and_kind_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-kind-and-user", get(dlq_stats_by_minute_and_kind_and_user))
        .route("/api/v1/notifications/dlq/stats/by-user-and-tenant",           get(dlq_stats_by_user_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-kind-and-day-and-tenant",  get(dlq_stats_by_kind_and_day_and_tenant))
        .route("/api/v1/notifications/dlq/stats/error-length-by-kind",        get(dlq_stats_error_length_by_kind))
        .route("/api/v1/notifications/dlq/stats/tenant-coverage",             get(dlq_stats_tenant_coverage))
        .route("/api/v1/notifications/dlq/stats/user-coverage",                   get(dlq_stats_user_coverage))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-day-and-kind",         get(dlq_stats_by_hour_and_day_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-day-and-user",         get(dlq_stats_by_hour_and_day_and_user))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-day-and-tenant",      get(dlq_stats_by_hour_and_day_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-day-and-hour-and-kind",        get(dlq_stats_by_day_and_hour_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-day",                 get(dlq_stats_by_hour_and_day))
        .route("/api/v1/notifications/dlq/stats/by-tenant-and-user-and-kind",     get(dlq_stats_by_tenant_and_user_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-user-and-hour",                get(dlq_stats_by_user_and_hour))
        .route("/api/v1/notifications/dlq/stats/by-tenant-and-day-and-kind",      get(dlq_stats_by_tenant_and_day_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-kind-and-user-and-day",        get(dlq_stats_by_kind_and_user_and_day))
        .route("/api/v1/notifications/dlq/stats/by-attempts-and-tenant",       get(dlq_stats_by_attempts_and_tenant))
        .route("/api/v1/notifications/dlq/stats/failed-at-hour-distribution",  get(dlq_stats_failed_at_hour_distribution))
        .route("/api/v1/notifications/dlq/stats/retry-rate-by-kind",          get(dlq_stats_retry_rate_by_kind))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-user-and-kind",   get(dlq_stats_by_hour_and_user_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-kind-and-user",   get(dlq_stats_by_hour_and_kind_and_user))
        .route("/api/v1/notifications/dlq/stats/by-user-and-day-and-hour",    get(dlq_stats_by_user_and_day_and_hour))
        .route("/api/v1/notifications/dlq/stats/by-tenant-and-hour-and-user", get(dlq_stats_by_tenant_and_hour_and_user))
        .route("/api/v1/notifications/dlq/stats/by-kind-and-hour-and-user",   get(dlq_stats_by_kind_and_hour_and_user))
        .route("/api/v1/notifications/dlq/stats/by-kind-and-tenant-and-hour", get(dlq_stats_by_kind_and_tenant_and_hour))
        .route("/api/v1/notifications/dlq/stats/by-user-and-kind-and-minute",   get(dlq_stats_by_user_and_kind_and_minute))
        .route("/api/v1/notifications/dlq/stats/by-tenant-and-kind-and-minute", get(dlq_stats_by_tenant_and_kind_and_minute))
        .route("/api/v1/notifications/dlq/stats/by-day-and-kind-and-user",      get(dlq_stats_by_day_and_kind_and_user))
        .route("/api/v1/notifications/dlq/stats/by-kind-and-user-and-hour",     get(dlq_stats_by_kind_and_user_and_hour))
        .route("/api/v1/notifications/dlq/stats/by-kind-and-day-and-hour",      get(dlq_stats_by_kind_and_day_and_hour))
        .route("/api/v1/notifications/dlq/stats/by-second-and-day",             get(dlq_stats_by_second_and_day))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-tenant-and-day",  get(dlq_stats_by_minute_and_tenant_and_day))
        .route("/api/v1/notifications/dlq/stats/by-day-and-second-and-kind",    get(dlq_stats_by_day_and_second_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-day-and-second-and-user",    get(dlq_stats_by_day_and_second_and_user))
        .route("/api/v1/notifications/dlq/stats/by-day-and-second-and-tenant",  get(dlq_stats_by_day_and_second_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-second-and-day-and-kind",    get(dlq_stats_by_second_and_day_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-second-and-day-and-user",    get(dlq_stats_by_second_and_day_and_user))
        .route("/api/v1/notifications/dlq/stats/by-second-and-day-and-tenant",  get(dlq_stats_by_second_and_day_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-day-and-kind",    get(dlq_stats_by_minute_and_day_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-day-and-user",    get(dlq_stats_by_minute_and_day_and_user))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-day-and-tenant",  get(dlq_stats_by_minute_and_day_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-second-and-kind-and-tenant",  get(dlq_stats_by_second_and_kind_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-tenant-and-kind",    get(dlq_stats_by_hour_and_tenant_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-kind-and-tenant",  get(dlq_stats_by_minute_and_kind_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-day-and-tenant-and-kind",     get(dlq_stats_by_day_and_tenant_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-second",   get(dlq_stats_by_hour_and_second))
        .route("/api/v1/notifications/dlq/stats/by-tenant-and-second", get(dlq_stats_by_tenant_and_second))
        .route("/api/v1/notifications/dlq/stats/by-kind-and-second",   get(dlq_stats_by_kind_and_second))
        .route("/api/v1/notifications/dlq/stats/by-user-and-second",   get(dlq_stats_by_user_and_second))
        .route("/api/v1/notifications/dlq/stats/by-second-and-minute",          get(dlq_stats_by_second_and_minute))
        .route("/api/v1/notifications/dlq/stats/by-day-and-minute-and-user",    get(dlq_stats_by_day_and_minute_and_user))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-minute",            get(dlq_stats_by_hour_and_minute))
        .route("/api/v1/notifications/dlq/stats/by-second-and-user-and-kind",   get(dlq_stats_by_second_and_user_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-second-and-kind", get(dlq_stats_by_minute_and_second_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-second-and-user",   get(dlq_stats_by_minute_and_second_and_user))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-second-and-tenant", get(dlq_stats_by_minute_and_second_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-second-and-minute-and-tenant", get(dlq_stats_by_second_and_minute_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-second-and-minute-and-user",   get(dlq_stats_by_second_and_minute_and_user))
        .route("/api/v1/notifications/dlq/stats/by-second-and-minute-and-kind",  get(dlq_stats_by_second_and_minute_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-second-and-kind",    get(dlq_stats_by_hour_and_second_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-second-and-user",    get(dlq_stats_by_hour_and_second_and_user))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-second-and-tenant",  get(dlq_stats_by_hour_and_second_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-minute-and-user",    get(dlq_stats_by_hour_and_minute_and_user))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-hour-and-kind",   get(dlq_stats_by_minute_and_hour_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-hour-and-user",   get(dlq_stats_by_minute_and_hour_and_user))
        .route("/api/v1/notifications/dlq/stats/by-minute-and-hour-and-tenant", get(dlq_stats_by_minute_and_hour_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-day-and-hour-and-tenant",    get(dlq_stats_by_day_and_hour_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-second-and-hour-and-kind",   get(dlq_stats_by_second_and_hour_and_kind))
        .route("/api/v1/notifications/dlq/stats/by-second-and-hour-and-user",   get(dlq_stats_by_second_and_hour_and_user))
        .route("/api/v1/notifications/dlq/stats/by-second-and-hour-and-tenant", get(dlq_stats_by_second_and_hour_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-day-and-hour-and-user",      get(dlq_stats_by_day_and_hour_and_user))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-minute-and-tenant",  get(dlq_stats_by_hour_and_minute_and_tenant))
        .route("/api/v1/notifications/dlq/stats/by-hour-and-minute-and-kind",    get(dlq_stats_by_hour_and_minute_and_kind))
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

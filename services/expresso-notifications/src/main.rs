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
//! For multi-pod deployments the `REDIS_URL` env var enables a Redis pub/sub
//! relay: a background task subscribes to "expresso:notifications" and rebroadcasts
//! into the in-process channel, so every pod sees every event.
//!
//! Ports:
//!   :8006  HTTP (configurable via HOST/PORT)

use std::{env, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    async_trait,
    extract::{FromRequestParts, Query, Request, State},
    http::request::Parts,
    middleware::{self, Next},
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::{get, post},
    Json, Router,
};
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
    tx:        Arc<broadcast::Sender<Notification>>,
    validator: Option<Arc<OidcValidator>>,
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
    let _ = st.tx.send(notif);
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

async fn health() -> Json<serde_json::Value> {
    Json(json!({"service": SERVICE, "status": "ok"}))
}

async fn ready() -> Json<serde_json::Value> {
    Json(json!({"ready": true}))
}

// ─── Redis relay (optional) ───────────────────────────────────────────────────

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

    let validator = maybe_build_validator().await;
    let state = AppState { tx, validator };

    let app = Router::new()
        .route("/health",               get(health))
        .route("/ready",                get(ready))
        .route("/internal/notify",      post(internal_notify))
        .route("/notifications/stream", get(notifications_stream))
        .merge(expresso_observability::metrics_router())
        .layer(middleware::from_fn_with_state(state.clone(), inject_validator))
        .with_state(state);

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(service = SERVICE, %addr, "listening");

    axum::serve(listener, app).await?;

    Ok(())
}

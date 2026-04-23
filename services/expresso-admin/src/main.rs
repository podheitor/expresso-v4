//! expresso-admin — SSR admin UI + health/metrics.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tower_http::services::ServeDir;
use tracing::info;

mod auth;
mod dav_admin;
mod handlers;
mod tenants;
mod audit;
mod counter;
mod dead_props;
mod drive_quotas;
mod kc;
mod templates;

use kc::KcClient;
use auth::AuthConfig;

const SERVICE: &str = "expresso-admin";
const DEFAULT_PORT: u16 = 8101;

pub struct AppState {
    pub kc:   KcClient,
    pub http: reqwest::Client,
    pub auth: AuthConfig,
    pub db:   Option<expresso_core::DbPool>,
}

pub struct AdminError(pub anyhow::Error);

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        tracing::error!(error = %self.0, "admin error");
        (StatusCode::INTERNAL_SERVER_ERROR, format!("upstream error: {}", self.0)).into_response()
    }
}

async fn health() -> Json<Value> { Json(json!({"service": SERVICE, "status": "ok"})) }
async fn ready()  -> Json<Value> { Json(json!({"ready": true})) }

fn resolve_addr() -> anyhow::Result<SocketAddr> {
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port = std::env::var("PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_PORT);
    Ok(format!("{host}:{port}").parse()?)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let db = match std::env::var("DATABASE__URL").ok() {
        Some(url) => {
            let cfg = expresso_core::config::DatabaseConfig {
                url,
                max_connections: std::env::var("DATABASE__MAX_CONNECTIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(10),
                min_connections: std::env::var("DATABASE__MIN_CONNECTIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(1),
                acquire_timeout_secs: std::env::var("DATABASE__ACQUIRE_TIMEOUT_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(10),
            };
            match expresso_core::create_db_pool(&cfg).await {
                Ok(pool) => Some(pool),
                Err(e) => { tracing::warn!(error=%e, "database unavailable"); None }
            }
        }
        None => { tracing::warn!("DATABASE__URL not set; DAV admin disabled"); None }
    };

    let state = Arc::new(AppState {
        kc:   handlers::kc_factory(),
        http: reqwest::Client::builder().build()?,
        auth: AuthConfig::from_env(),
        db,
    });

    let app = Router::new()
        .route("/",       get(handlers::dashboard))
        .route("/users",  get(handlers::users))
        .route("/users/new", get(handlers::user_new).post(handlers::user_create))
        .route("/users/:id/edit", get(handlers::user_edit).post(handlers::user_update))
        .route("/users/:id/delete", post(handlers::user_delete))
        .route("/realm",  get(handlers::realm_page))
        .route("/calendars", get(dav_admin::calendars_list))
        .route("/calendars/:tenant_id/:id/edit", get(dav_admin::calendar_edit_form).post(dav_admin::calendar_edit_action))
        .route("/calendars/:tenant_id/:id/delete", post(dav_admin::calendar_delete_action))
        .route("/addressbooks", get(dav_admin::addressbooks_list))
        .route("/addressbooks/:tenant_id/:id/edit", get(dav_admin::addressbook_edit_form).post(dav_admin::addressbook_edit_action))
        .route("/addressbooks/:tenant_id/:id/delete", post(dav_admin::addressbook_delete_action))
        .route("/tenants",                  get(tenants::list))
        .route("/tenants/new",              get(tenants::new_form).post(tenants::create_action))
        .route("/tenants/:id/edit",         get(tenants::edit_form).post(tenants::edit_action))
        .route("/tenants/:id/config",       get(tenants::config_form).post(tenants::config_action))
        .route("/tenants/:id/delete",       post(tenants::delete_action))
        .route("/audit.json",               get(audit::list))
        .route("/audit.html",               get(audit::page))
        .route("/audit",                    get(audit::page))
        .route("/audit/purge",              post(audit::purge))
        .route("/counter.html",             get(counter::page))
        .route("/counter/:id/accept",       post(counter::accept))
        .route("/counter/:id/reject",       post(counter::reject))
        .route("/dead-props.html",          get(dead_props::page))
        .route("/drive-quotas.html",        get(drive_quotas::page))
        .route("/drive-quotas/:tenant_id",  post(drive_quotas::update))
        .route("/health", get(health))
        .route("/ready",  get(ready))
        .nest_service("/static", ServeDir::new("static"))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth::require_admin))
        .with_state(state)
        .merge(expresso_observability::metrics_router());

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(service = SERVICE, %addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

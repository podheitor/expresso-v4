//! expresso-auth — OIDC Relying Party service.
//!
//! Routes:
//!   GET  /auth/login     → 302 to IdP authorization_endpoint (PKCE S256)
//!   GET  /auth/callback  → exchange code + return TokenResponse
//!   POST /auth/refresh   → refresh_token flow
//!   GET  /auth/logout    → 302 to IdP end_session_endpoint
//!   GET  /auth/me        → validated AuthContext (Bearer required)
//!   GET  /health /ready  → liveness/readiness

mod config;
mod ratelimit;
mod error;
mod handlers;
mod kc_admin;
mod oidc;
mod state;

use std::{env, net::SocketAddr, sync::Arc, collections::HashMap};

use axum::{
    extract::Extension,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::info;

use expresso_auth_client::{MultiRealmValidator, OidcConfig, OidcValidator, TenantResolver};

use crate::{
    config::RpConfig,
    oidc::discovery::ProviderMetadata,
    state::AppState,
};

const SERVICE: &str = "expresso-auth";
const DEFAULT_PORT: u16 = 8100;

async fn health() -> Json<Value> { Json(json!({"service": SERVICE, "status": "ok"})) }
async fn ready()  -> Json<Value> { Json(json!({"ready": true})) }

fn resolve_addr() -> anyhow::Result<SocketAddr> {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_PORT);
    format!("{}:{}", host, port).parse::<SocketAddr>()
        .map_err(|e| anyhow::anyhow!("invalid bind address: {}", e))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let cfg = RpConfig::from_env()?;

    // Discover IdP endpoints
    let provider = ProviderMetadata::fetch(&cfg.issuer, cfg.http_timeout).await
        .map_err(|e| anyhow::anyhow!("discovery: {e}"))?;
    info!(issuer = %provider.issuer, "provider metadata loaded");

    // Token validator (JWKS-backed) — reuses same issuer/audience config
    let validator_cfg = OidcConfig::new(cfg.issuer.clone(), cfg.client_id.clone());
    let validator = Arc::new(
        OidcValidator::new(validator_cfg).await
            .map_err(|e| anyhow::anyhow!("validator init: {e}"))?
    );
    info!("JWKS loaded");

    let http = reqwest::Client::builder()
        .timeout(cfg.http_timeout)
        .build()?;

    // Optional DB pool for audit log writes.
    let pool = match env::var("DATABASE_URL").or_else(|_| env::var("DATABASE__URL")) {
        Ok(url) if !url.is_empty() => match sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(&url).await {
                Ok(p) => { info!("audit pool ready"); Some(p) }
                Err(e) => { tracing::warn!(error=%e, "audit pool unavailable (continuing without audit)"); None }
            },
        _ => { info!("DATABASE_URL unset → audit writes disabled"); None }
    };

    let app_state = Arc::new(AppState {
        cfg,
        provider,
        http,
        validator: validator.clone(),
        pending: Mutex::new(HashMap::new()),
        pool,
    });

    let login_limiter = std::sync::Arc::new(
        ratelimit::RateLimiter::new(std::time::Duration::from_secs(60), 20)
    );

    // Multi-realm opt-in (fase 2 do realm-per-tenant). Ativo quando
    // AUTH__OIDC_ISSUER_TEMPLATE + AUTH__TENANT_HOSTS setados.
    let (multi, tenant_resolver): (Option<Arc<MultiRealmValidator>>, Option<Arc<TenantResolver>>) = {
        let tpl = env::var("AUTH__OIDC_ISSUER_TEMPLATE").ok().filter(|v| !v.is_empty());
        let aud = env::var("AUTH__OIDC_AUDIENCE").ok().filter(|v| !v.is_empty());
        match (tpl, aud) {
            (Some(t), Some(a)) => {
                let r = TenantResolver::from_env("AUTH__TENANT_HOSTS");
                if r.is_empty() {
                    tracing::warn!("AUTH__TENANT_HOSTS empty — multi-realm disabled");
                    (None, None)
                } else {
                    match MultiRealmValidator::new(t.clone(), a.clone()) {
                        Ok(m)  => { info!(template = %t, hosts = r.len(), "multi-realm validator ready"); (Some(Arc::new(m)), Some(Arc::new(r))) }
                        Err(e) => { tracing::error!(error = %e, "multi-realm init failed"); (None, None) }
                    }
                }
            }
            _ => (None, None),
        }
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready",  get(ready))
        .route("/auth/login",    get(handlers::login::login)
            .layer(axum::middleware::from_fn_with_state(login_limiter.clone(), ratelimit::rate_limit_mw)))
        .route("/auth/callback", get(handlers::callback::callback))
        .route("/auth/refresh",  post(handlers::refresh::refresh))
        .route("/auth/logout",   get(handlers::logout::logout))
        .route("/auth/me",       get(handlers::me::me))
        .route("/auth/impersonate/end", post(handlers::impersonate::end))
        .route("/auth/impersonate/:target_user_id", post(handlers::impersonate::start))
        .route("/auth/forgot", post(handlers::forgot::forgot))
        .merge(expresso_observability::metrics_router())
        .with_state(app_state)
        // Extension for Authenticated extractor (/auth/me)
        .layer(Extension(validator));
    let app = match multi     { Some(m) => app.layer(Extension(m)), None => app };
    let app = match tenant_resolver { Some(r) => app.layer(Extension(r)), None => app };

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(service = SERVICE, %addr, "listening");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}

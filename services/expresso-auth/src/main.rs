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
mod error;
mod handlers;
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

use expresso_auth_client::{OidcConfig, OidcValidator};

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

    let app_state = Arc::new(AppState {
        cfg,
        provider,
        http,
        validator: validator.clone(),
        pending: Mutex::new(HashMap::new()),
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready",  get(ready))
        .route("/auth/login",    get(handlers::login::login))
        .route("/auth/callback", get(handlers::callback::callback))
        .route("/auth/refresh",  post(handlers::refresh::refresh))
        .route("/auth/logout",   get(handlers::logout::logout))
        .route("/auth/me",       get(handlers::me::me))
        .with_state(app_state)
        // Extension for Authenticated extractor (/auth/me)
        .layer(Extension(validator));

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(service = SERVICE, %addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

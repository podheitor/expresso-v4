//! expresso-search — full-text search service backed by Tantivy.
//!
//! Endpoints:
//!   GET  /health                     → service health (open)
//!   GET  /ready                      → readiness     (open)
//!   POST /api/v1/index               → index document       (auth)
//!   GET  /api/v1/search?q=&tenant_id=  → search             (auth)
//!   DELETE /api/v1/index/:id         → remove document      (auth)
//!
//! Auth: quando `SEARCH_SERVICE_TOKEN` estiver no env, todos os endpoints
//! /api/v1/* exigem `Authorization: Bearer <token>` (compare em tempo
//! constante). Em dev a var pode ficar vazia — log de WARN no startup,
//! sem auth aplicada. Health/ready/metrics ficam abertos sempre (probes
//! de orquestrador + Prometheus).

mod api;
mod index_store;

use std::{env, net::SocketAddr, path::PathBuf};

use axum::{
    extract::Request,
    http::{header, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tracing::{info, warn};

use index_store::IndexStore;

const SERVICE: &str = "expresso-search";
const DEFAULT_PORT: u16 = 8007;

/// Constant-time byte compare. Length difference returns false sem compare
/// — tokens têm tamanho fixo conhecido pelo deployer, então length leak é
/// aceitável; o que importa é não vazar prefixo via timing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn require_bearer_token(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Token vem do State via extension — set abaixo no main().
    let expected = req
        .extensions()
        .get::<ServiceToken>()
        .map(|t| t.0.clone())
        .unwrap_or_default();

    let supplied = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");

    if ct_eq(expected.as_bytes(), supplied.as_bytes()) {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

#[derive(Clone)]
struct ServiceToken(String);

async fn health() -> Json<Value> {
    Json(json!({"service": SERVICE, "status": "ok"}))
}

async fn ready() -> Json<Value> {
    Json(json!({"ready": true}))
}

fn resolve_addr() -> anyhow::Result<SocketAddr> {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    Ok(format!("{}:{}", host, port).parse()?)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let data_dir = env::var("SEARCH_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/var/lib/expresso/search"));

    let store = IndexStore::open(&data_dir)?;

    let token = env::var("SEARCH_SERVICE_TOKEN").unwrap_or_default();
    if token.is_empty() {
        warn!(
            service = SERVICE,
            "SEARCH_SERVICE_TOKEN not set — /api/v1/* endpoints exposed without auth (dev mode)"
        );
    } else {
        info!(service = SERVICE, "Bearer-token auth enabled on /api/v1/*");
    }

    let api_routes = Router::new()
        .route("/api/v1/index", post(api::index_doc))
        .route("/api/v1/index/{id}", delete(api::delete_doc))
        .route("/api/v1/search", get(api::search))
        .with_state(store);

    let api_routes = if !token.is_empty() {
        let tok = ServiceToken(token);
        api_routes.layer(axum::Extension(tok))
            .layer(middleware::from_fn(require_bearer_token))
    } else {
        api_routes
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .merge(api_routes)
        .merge(expresso_observability::metrics_router());

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(service = SERVICE, %addr, "listening");
    axum::serve(listener, app).await?;

    Ok(())
}

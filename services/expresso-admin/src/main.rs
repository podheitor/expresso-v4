//! expresso-admin — SSR admin UI + health/metrics.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use tower_http::services::ServeDir;
use tracing::info;

mod handlers;
mod kc;
mod templates;

use kc::KcClient;

const SERVICE: &str = "expresso-admin";
const DEFAULT_PORT: u16 = 8101;

pub struct AppState {
    pub kc: KcClient,
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

    let state = Arc::new(AppState { kc: handlers::kc_factory() });

    let app = Router::new()
        .route("/",       get(handlers::dashboard))
        .route("/users",  get(handlers::users))
        .route("/realm",  get(handlers::realm_page))
        .route("/health", get(health))
        .route("/ready",  get(ready))
        .nest_service("/static", ServeDir::new("static"))
        .with_state(state)
        .merge(expresso_observability::metrics_router());

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(service = SERVICE, %addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

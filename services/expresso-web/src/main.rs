//! expresso-web — SSR UI (Axum + Askama + reqwest upstream proxy)

mod config;
mod error;
mod routes;
mod templates;
mod upstream;

use std::{env, net::SocketAddr, sync::Arc, time::Duration};
use tower_http::services::ServeDir;
use tracing::{info, warn};

use expresso_core::{init_tracing, config::TelemetryConfig};

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16  = 8080;
const DEFAULT_OTLP: &str = "http://localhost:4317";
const DEFAULT_FILTER: &str = "info";

fn env_string(k: &str) -> Option<String> { env::var(k).ok().filter(|v| !v.trim().is_empty()) }
fn env_u16(k: &str, d: u16) -> u16 { env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }
fn env_bool(k: &str, d: bool) -> bool { env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }

#[derive(Clone)]
pub struct AppState {
    pub http:     reqwest::Client,
    pub backends: Arc<config::Backends>,
    pub public:   Arc<config::Public>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing(&TelemetryConfig {
        otlp_endpoint: env_string("TELEMETRY__OTLP_ENDPOINT").unwrap_or_else(|| DEFAULT_OTLP.into()),
        log_json:      env_bool("TELEMETRY__LOG_JSON", false),
        log_filter:    env_string("TELEMETRY__LOG_FILTER").unwrap_or_else(|| DEFAULT_FILTER.into()),
    });
    info!(version = env!("CARGO_PKG_VERSION"), "expresso-web starting");

    let backends = config::Backends::from_env();
    let public   = config::Public::from_env();
    info!(?backends, ?public, "config loaded");

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .pool_idle_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let state = AppState { http, backends: Arc::new(backends), public: Arc::new(public) };

    let static_dir = env_string("WEB__STATIC_DIR").unwrap_or_else(|| "static".into());
    let app = routes::router(state).nest_service("/static", ServeDir::new(&static_dir));

    let host = env_string("SERVER__HOST").unwrap_or_else(|| DEFAULT_HOST.into());
    let port = env_u16("SERVER__PORT", DEFAULT_PORT);
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let lst = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "HTTP listening");
    if let Err(e) = axum::serve(lst, app).await { warn!(error=%e, "serve failed"); }
    Ok(())
}

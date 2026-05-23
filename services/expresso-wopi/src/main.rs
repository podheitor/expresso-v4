//! expresso-wopi service entrypoint

use std::{env, net::SocketAddr};

use axum::{routing::get, Json, Router};
use serde_json::{json, Value};
use tracing::info;

const SERVICE: &str = "expresso-wopi";
const DEFAULT_PORT: u16 = 8008;

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

    let addr = format!("{}:{}", host, port)
        .parse::<SocketAddr>()
        .map_err(|e| anyhow::anyhow!("invalid bind address: {}", e))?;

    Ok(addr)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .merge(expresso_observability::metrics_router());
    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(service = SERVICE, %addr, "listening");

    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_addr_default_port_when_env_unset() {
        std::env::remove_var("PORT");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), DEFAULT_PORT);
    }

    #[test]
    fn resolve_addr_custom_port() {
        std::env::set_var("PORT", "9008");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), 9008);
        std::env::remove_var("PORT");
    }

    #[test]
    fn resolve_addr_default_host_is_all_interfaces() {
        std::env::remove_var("HOST");
        std::env::remove_var("PORT");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.ip().to_string(), "0.0.0.0");
    }

    #[test]
    fn resolve_addr_invalid_port_uses_default() {
        std::env::set_var("PORT", "not_a_port");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), DEFAULT_PORT);
        std::env::remove_var("PORT");
    }

    #[test]
    fn default_port_is_8008() {
        assert_eq!(DEFAULT_PORT, 8008);
    }

    #[test]
    fn service_name_is_expresso_wopi() {
        assert_eq!(SERVICE, "expresso-wopi");
    }

    #[test]
    fn resolve_addr_loopback_host() {
        std::env::set_var("HOST", "127.0.0.1");
        std::env::remove_var("PORT");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        std::env::remove_var("HOST");
    }

    #[test]
    fn resolve_addr_returns_ok() {
        std::env::remove_var("PORT");
        std::env::remove_var("HOST");
        assert!(resolve_addr().is_ok());
    }

    #[test]
    fn resolve_addr_port_65535() {
        std::env::set_var("PORT", "65535");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), 65535);
        std::env::remove_var("PORT");
    }

    #[test]
    fn resolve_addr_host_env_overrides_default() {
        std::env::set_var("HOST", "192.168.1.1");
        std::env::remove_var("PORT");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.ip().to_string(), "192.168.1.1");
        std::env::remove_var("HOST");
    }

    #[test]
    fn resolve_addr_port_8080() {
        std::env::set_var("PORT", "8080");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), 8080);
        std::env::remove_var("PORT");
    }

    #[test]
    fn resolve_addr_port_8008_explicit() {
        std::env::set_var("PORT", "8008");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), 8008);
        std::env::remove_var("PORT");
    }

    #[test]
    fn resolve_addr_port_1() {
        std::env::set_var("PORT", "1");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), 1);
        std::env::remove_var("PORT");
    }
}

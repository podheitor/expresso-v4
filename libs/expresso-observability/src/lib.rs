// expresso-observability — shared Prometheus metrics + axum /metrics route
//
// Usage:
//   let router = Router::new()
//       .merge(expresso_observability::metrics_router())
//       .route(...);
//
// Custom metrics: register into expresso_observability::registry()

use axum::{http::StatusCode, response::IntoResponse, routing::get, Router};
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounterVec, Registry, TextEncoder};

// Global registry — single source across service + custom metrics
// Use default prometheus registry so metrics registered via
// `register_int_counter_vec!` / `register_int_gauge!` from any lib show up.
fn registry_ref() -> &'static Registry { prometheus::default_registry() }

// Built-in HTTP request counter (opt-in via middleware — not auto-wired)
pub static HTTP_REQUESTS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new("http_requests_total", "Total HTTP requests"),
        &["service", "method", "status"],
    )
    .expect("metric build");
    registry_ref().register(Box::new(c.clone())).expect("metric register");
    c
});

// Access global registry to register custom metrics
pub fn registry() -> &'static Registry {
    registry_ref()
}

// Register counter/histogram into global registry — convenience wrapper
pub fn register<T: prometheus::core::Collector + Clone + 'static>(metric: T) -> T {
    registry_ref()
        .register(Box::new(metric.clone()))
        .expect("metric register");
    metric
}

async fn metrics_handler() -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = registry_ref().gather();
    let mut buf = Vec::new();
    if let Err(e) = encoder.encode(&metric_families, &mut buf) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("encode err: {e}")).into_response();
    }
    match String::from_utf8(buf) {
        Ok(s) => (StatusCode::OK, [("content-type", encoder.format_type())], s).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "utf8").into_response(),
    }
}

// Router exposing GET /metrics — merge into service root router
pub fn metrics_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    // Force lazy init so built-in counter appears even before first use
    Lazy::force(&HTTP_REQUESTS_TOTAL);
    Router::new().route("/metrics", get(metrics_handler))
}


// HTTP request counter middleware. Labels: service / method / status.
// Usage: `app.layer(axum::middleware::from_fn(expresso_observability::http_counter_mw))`.
pub async fn http_counter_mw(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = req.method().to_string();
    let service = std::env::var("EXPRESSO_SERVICE_NAME")
        .unwrap_or_else(|_| "expresso".into());
    let resp = next.run(req).await;
    let status = resp.status().as_u16().to_string();
    HTTP_REQUESTS_TOTAL
        .with_label_values(&[&service, &method, &status])
        .inc();
    resp
}

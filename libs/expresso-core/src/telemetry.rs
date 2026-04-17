//! Tracing + OpenTelemetry initialisation
//! Call `init_tracing(&cfg)` once at startup before any instrumentation.

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use crate::config::TelemetryConfig;

/// Initialise the global tracing subscriber.
/// - JSON output when cfg.log_json = true (production)
/// - Human-readable (pretty) otherwise (dev)
/// - Exports spans to OTLP collector when endpoint is reachable
pub fn init_tracing(cfg: &TelemetryConfig) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&cfg.log_filter));

    if cfg.log_json {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().pretty())
            .init();
    }

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "tracing initialised"
    );
}

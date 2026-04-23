// expresso-event-audit: NATS JetStream consumer.
// Subscribes to calendar + contacts event streams and logs each message
// as structured JSON via tracing. Audit/debug role; zero business logic.
//
// Env:
//   NATS_URL            = nats://host:4222   (required)
//   NATS_DURABLE        = consumer name      (default: event-audit)
//   NATS_SUBJECT_FILTER = subject pattern    (default: expresso.>)
//   RUST_LOG            = tracing filter     (default: info)
//   METRICS_ADDR        = bind for ops http  (default: 0.0.0.0:9090)

use anyhow::{Context, Result};
use async_nats::jetstream::{
    self,
    consumer::{pull::Config as PullConfig, DeliverPolicy},
};
use axum::{response::IntoResponse, routing::get, Router};
use futures::StreamExt;
use once_cell::sync::Lazy;
use prometheus::{register_int_counter_vec, Encoder, IntCounterVec, TextEncoder};
use std::env;
use tracing::{error, info, warn};

// Counter: events observed per stream. Used by Grafana (sprint #21) for
// consumer health + throughput panels.
static EVENTS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "event_audit_events_total",
        "Total events consumed per stream by expresso-event-audit.",
        &["stream"]
    )
    .expect("register counter")
});

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let nats_url = env::var("NATS_URL").context("NATS_URL required")?;
    let durable = env::var("NATS_DURABLE").unwrap_or_else(|_| "event-audit".into());
    let subject_filter =
        env::var("NATS_SUBJECT_FILTER").unwrap_or_else(|_| "expresso.>".into());
    let metrics_addr = env::var("METRICS_ADDR").unwrap_or_else(|_| "0.0.0.0:9090".into());

    // Spawn ops HTTP (healthz + metrics) first; survives NATS outages.
    tokio::spawn(run_ops_http(metrics_addr.clone()));

    info!(%nats_url, %durable, %subject_filter, %metrics_addr, "connecting");
    let client = async_nats::connect(&nats_url).await?;
    let js = jetstream::new(client);

    for stream_name in ["EXPRESSO_CALENDAR", "EXPRESSO_CONTACTS"] {
        let js = js.clone();
        let durable = format!("{durable}-{}", stream_name.to_lowercase());
        let filter = subject_filter.clone();
        tokio::spawn(async move {
            if let Err(e) = run_consumer(js, stream_name, &durable, &filter).await {
                error!(stream = %stream_name, error = %e, "consumer exited");
            }
        });
    }

    tokio::signal::ctrl_c().await?;
    info!("shutdown");
    Ok(())
}

async fn run_ops_http(addr: String) {
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(|| async { "ok" }))
        .route("/metrics", get(metrics_handler));
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            error!(error = %e, %addr, "ops http bind failed");
            return;
        }
    };
    info!(%addr, "ops http ready");
    if let Err(e) = axum::serve(listener, app).await {
        error!(error = %e, "ops http exited");
    }
}

async fn metrics_handler() -> impl IntoResponse {
    let mut buf = Vec::new();
    let enc = TextEncoder::new();
    if let Err(e) = enc.encode(&prometheus::gather(), &mut buf) {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("encode: {e}"),
        )
            .into_response();
    }
    (
        [(axum::http::header::CONTENT_TYPE, enc.format_type())],
        buf,
    )
        .into_response()
}

async fn run_consumer(
    js: jetstream::Context,
    stream_name: &str,
    durable: &str,
    filter: &str,
) -> Result<()> {
    let stream = match js.get_stream(stream_name).await {
        Ok(s) => s,
        Err(e) => {
            warn!(stream = %stream_name, error = %e, "stream missing; skip");
            return Ok(());
        }
    };

    let consumer = stream
        .get_or_create_consumer(
            durable,
            PullConfig {
                durable_name: Some(durable.into()),
                filter_subject: filter.into(),
                deliver_policy: DeliverPolicy::New,
                ..Default::default()
            },
        )
        .await
        .context("create consumer")?;

    info!(stream = %stream_name, %durable, %filter, "consumer ready");

    let mut messages = consumer.messages().await?;
    while let Some(msg) = messages.next().await {
        match msg {
            Ok(m) => {
                let subject = m.subject.as_str();
                let payload = String::from_utf8_lossy(&m.payload);
                info!(
                    target: "event_audit",
                    stream = %stream_name,
                    subject = %subject,
                    payload = %payload,
                    "event"
                );
                EVENTS_TOTAL.with_label_values(&[stream_name]).inc();
                if let Err(e) = m.ack().await {
                    warn!(error = %e, "ack failed");
                }
            }
            Err(e) => {
                warn!(error = %e, "recv error");
            }
        }
    }
    Ok(())
}

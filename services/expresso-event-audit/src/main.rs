// expresso-event-audit: NATS JetStream consumer.
// Subscribes to calendar + contacts event streams and logs each message
// as structured JSON via tracing. Audit/debug role; zero business logic.
//
// Env:
//   NATS_URL           = nats://host:4222   (required)
//   NATS_DURABLE       = consumer name      (default: event-audit)
//   NATS_SUBJECT_FILTER = subject pattern   (default: expresso.>)
//   RUST_LOG           = tracing filter     (default: info)

use anyhow::{Context, Result};
use async_nats::jetstream::{
    self,
    consumer::{pull::Config as PullConfig, DeliverPolicy},
};
use futures::StreamExt;
use std::env;
use tracing::{error, info, warn};

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

    info!(%nats_url, %durable, %subject_filter, "connecting");
    let client = async_nats::connect(&nats_url).await?;
    let js = jetstream::new(client);

    // Bind to BOTH streams; pick whichever covers the subject filter.
    // Stream names hard-coded to match calendar/contacts publishers.
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

    // Block forever; consumers run in spawned tasks.
    tokio::signal::ctrl_c().await?;
    info!("shutdown");
    Ok(())
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

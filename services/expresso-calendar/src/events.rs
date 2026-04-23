//! In-process event bus for real-time calendar notifications.
//!
//! MVP: `tokio::sync::broadcast` channel. Consumers subscribe via SSE
//! endpoint (`GET /api/v1/events/stream`), filtered by tenant.
//!
//! Sprint #20: optional NATS JetStream publish on top of broadcast.
//! When `EventBus::new_with_nats(url)` is used, every published `Event` is
//! also fire-and-forget published to subject
//! `expresso.calendar.<tenant_id>.<kind>` on stream `EXPRESSO_CALENDAR`.

use serde::Serialize;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Bounded channel capacity. Lagging receivers drop oldest events.
const BUS_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    EventCreated {
        tenant_id: Uuid,
        event_id:  Uuid,
        summary:   Option<String>,
    },
    EventUpdated {
        tenant_id: Uuid,
        event_id:  Uuid,
        summary:   Option<String>,
        sequence:  i32,
    },
    EventCancelled {
        tenant_id: Uuid,
        event_id:  Uuid,
    },
    CounterReceived {
        tenant_id:      Uuid,
        event_id:       Uuid,
        proposal_id:    Uuid,
        attendee_email: String,
    },
}

impl Event {
    pub fn tenant_id(&self) -> Uuid {
        match self {
            Event::EventCreated    { tenant_id, .. } => *tenant_id,
            Event::EventUpdated    { tenant_id, .. } => *tenant_id,
            Event::EventCancelled  { tenant_id, .. } => *tenant_id,
            Event::CounterReceived { tenant_id, .. } => *tenant_id,
        }
    }

    pub fn kind_str(&self) -> &'static str {
        match self {
            Event::EventCreated    { .. } => "event_created",
            Event::EventUpdated    { .. } => "event_updated",
            Event::EventCancelled  { .. } => "event_cancelled",
            Event::CounterReceived { .. } => "counter_received",
        }
    }
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
    /// Optional JetStream context — None = in-process only.
    jetstream: Option<async_nats::jetstream::Context>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx, jetstream: None }
    }

    /// Connect to NATS, ensure stream, return bus with JetStream publishing enabled.
    pub async fn new_with_nats(url: &str) -> anyhow::Result<Self> {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        let client = async_nats::connect(url).await?;
        let js = async_nats::jetstream::new(client);
        // Idempotent stream creation. subject = expresso.calendar.*.*
        let cfg = async_nats::jetstream::stream::Config {
            name: "EXPRESSO_CALENDAR".to_string(),
            subjects: vec!["expresso.calendar.>".to_string()],
            max_age: std::time::Duration::from_secs(60 * 60 * 24 * 7),
            ..Default::default()
        };
        js.get_or_create_stream(cfg).await
            .map_err(|e| anyhow::anyhow!("jetstream ensure: {e}"))?;
        tracing::info!(nats_url=%url, "jetstream EXPRESSO_CALENDAR ready");
        Ok(Self { tx, jetstream: Some(js) })
    }

    /// Best-effort publish to broadcast + (optional) JetStream.
    pub fn publish(&self, ev: Event) {
        let _ = self.tx.send(ev.clone());
        if let Some(js) = self.jetstream.clone() {
            let subject = format!("expresso.calendar.{}.{}", ev.tenant_id(), ev.kind_str());
            tokio::spawn(async move {
                match serde_json::to_vec(&ev) {
                    Ok(payload) => {
                        if let Err(e) = js.publish(subject.clone(), payload.into()).await {
                            tracing::warn!(error=%e, %subject, "nats publish failed");
                        }
                    }
                    Err(e) => tracing::warn!(error=%e, "event serialize failed"),
                }
            });
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self { Self::new() }
}

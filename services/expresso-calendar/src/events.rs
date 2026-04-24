//! In-process event bus for real-time calendar notifications.
//!
//! MVP: `tokio::sync::broadcast` channel. Consumers subscribe via SSE
//! endpoint (`GET /api/v1/events/stream`), filtered by tenant.
//!
//! Sprint #20: optional NATS JetStream publish on top of broadcast.
//! When `EventBus::new_with_nats(url)` is used, every published `Event` is
//! also fire-and-forget published to subject
//! `expresso.calendar.<tenant_id>.<kind>` on stream `EXPRESSO_CALENDAR`.

use once_cell::sync::Lazy;
use prometheus::IntCounterVec;
use serde::Serialize;

/// Publish attempts counter per {stream, kind, result}.
/// result ∈ {"ok","err","serialize_err"}. Populated by `EventBus::publish`.
/// Registered into shared `expresso_observability::registry()` so it appears
/// at GET /metrics of the service.
static NATS_PUBLISH_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            "calendar_nats_publish_total",
            "Calendar NATS publish attempts per kind and result",
        ),
        &["kind", "result"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

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
        Lazy::force(&NATS_PUBLISH_TOTAL);
        // Pre-populate zero-valued series so rate() works before first publish.
        for kind in ["event_created","event_updated","event_cancelled","counter_received"] {
            for result in ["ok","err","serialize_err"] {
                NATS_PUBLISH_TOTAL.with_label_values(&[kind, result]).inc_by(0);
            }
        }
        tracing::info!(nats_url=%url, "jetstream EXPRESSO_CALENDAR ready");
        Ok(Self { tx, jetstream: Some(js) })
    }

    /// Best-effort publish to broadcast + (optional) JetStream.
    pub fn publish(&self, ev: Event) {
        let _ = self.tx.send(ev.clone());
        if let Some(js) = self.jetstream.clone() {
            let kind = ev.kind_str();
            let subject = format!("expresso.calendar.{}.{}", ev.tenant_id(), kind);
            tokio::spawn(async move {
                match serde_json::to_vec(&ev) {
                    Ok(payload) => {
                        match js.publish(subject.clone(), payload.into()).await {
                            Ok(_) => {
                                NATS_PUBLISH_TOTAL.with_label_values(&[kind, "ok"]).inc();
                            }
                            Err(e) => {
                                NATS_PUBLISH_TOTAL.with_label_values(&[kind, "err"]).inc();
                                tracing::warn!(error=%e, %subject, "nats publish failed");
                            }
                        }
                    }
                    Err(e) => {
                        NATS_PUBLISH_TOTAL.with_label_values(&[kind, "serialize_err"]).inc();
                        tracing::warn!(error=%e, "event serialize failed");
                    }
                }
            });
        }
    }


    /// Publish iMIP envelope to `expresso.imip.request` for the given stored event.
    /// Fire-and-forget; silently skipped when JetStream not connected or event
    /// lacks attendees / dtstart / dtend / organizer_email. Returns `true`
    /// when the publish task was enqueued (JetStream connected); `false`
    /// when there is no JetStream context to publish through.
    pub fn publish_imip(&self, ev: crate::domain::event::Event, method: &'static str) -> bool {
        if let Some(js) = self.jetstream.clone() {
            crate::imip_publish::publish_imip(js, ev, method);
            true
        } else {
            false
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self { Self::new() }
}

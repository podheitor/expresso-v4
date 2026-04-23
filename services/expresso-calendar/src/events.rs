//! In-process event bus for real-time calendar notifications.
//!
//! MVP: `tokio::sync::broadcast` channel. Consumers subscribe via SSE
//! endpoint (`GET /api/v1/events/stream`), filtered by tenant.
//! Future: back with NATS when multi-node deployment required — the `Event`
//! enum shape is stable so only the transport swaps.

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
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    /// Best-effort publish. Returns silently if there are no subscribers.
    pub fn publish(&self, ev: Event) {
        let _ = self.tx.send(ev);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self { Self::new() }
}

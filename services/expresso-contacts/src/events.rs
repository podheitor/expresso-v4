//! Contacts event publication — sprint #23.
//!
//! Mirrors the calendar EventBus minus the in-process broadcast (contacts
//! has no SSE consumer yet). Publishes structured events to NATS JetStream
//! stream `EXPRESSO_CONTACTS` with subject
//! `expresso.contacts.<tenant_id>.<kind>`.

use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContactsEvent {
    AddressbookCreated { tenant_id: Uuid, addressbook_id: Uuid, name: Option<String> },
    AddressbookDeleted { tenant_id: Uuid, addressbook_id: Uuid },
    ContactUpserted    { tenant_id: Uuid, addressbook_id: Uuid, contact_id: Uuid },
    ContactDeleted     { tenant_id: Uuid, addressbook_id: Uuid, contact_id: Uuid },
}

impl ContactsEvent {
    pub fn tenant_id(&self) -> Uuid {
        match self {
            Self::AddressbookCreated { tenant_id, .. } => *tenant_id,
            Self::AddressbookDeleted { tenant_id, .. } => *tenant_id,
            Self::ContactUpserted    { tenant_id, .. } => *tenant_id,
            Self::ContactDeleted     { tenant_id, .. } => *tenant_id,
        }
    }
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::AddressbookCreated { .. } => "addressbook_created",
            Self::AddressbookDeleted { .. } => "addressbook_deleted",
            Self::ContactUpserted    { .. } => "contact_upserted",
            Self::ContactDeleted     { .. } => "contact_deleted",
        }
    }
}

#[derive(Clone)]
pub struct ContactsEventBus {
    jetstream: Option<async_nats::jetstream::Context>,
}

impl ContactsEventBus {
    pub fn noop() -> Self { Self { jetstream: None } }

    pub async fn new_with_nats(url: &str) -> anyhow::Result<Self> {
        let client = async_nats::connect(url).await?;
        let js = async_nats::jetstream::new(client);
        let cfg = async_nats::jetstream::stream::Config {
            name: "EXPRESSO_CONTACTS".to_string(),
            subjects: vec!["expresso.contacts.>".to_string()],
            max_age: std::time::Duration::from_secs(60 * 60 * 24 * 7),
            ..Default::default()
        };
        js.get_or_create_stream(cfg).await
            .map_err(|e| anyhow::anyhow!("jetstream ensure: {e}"))?;
        tracing::info!(nats_url=%url, "jetstream EXPRESSO_CONTACTS ready");
        Ok(Self { jetstream: Some(js) })
    }

    /// Best-effort fire-and-forget publish.
    pub fn publish(&self, ev: ContactsEvent) {
        let Some(js) = self.jetstream.clone() else { return; };
        let subject = format!("expresso.contacts.{}.{}", ev.tenant_id(), ev.kind_str());
        tokio::spawn(async move {
            match serde_json::to_vec(&ev) {
                Ok(payload) => {
                    if let Err(e) = js.publish(subject.clone(), payload.into()).await {
                        tracing::warn!(error=%e, %subject, "nats publish failed");
                    }
                }
                Err(e) => tracing::warn!(error=%e, "contacts event serialize failed"),
            }
        });
    }
}

impl Default for ContactsEventBus {
    fn default() -> Self { Self::noop() }
}

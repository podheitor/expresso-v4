//! Fire-and-forget webhook dispatch for meeting lifecycle events.
//!
//! Set `MEET__WEBHOOK_URL` to receive POST callbacks on meeting events.
//! Payload: `{"event": "<kind>", "tenant_id": "...", "meeting": {...}}`.
//! Delivery is best-effort — failures are logged but never propagate to callers.

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use tracing::warn;
use uuid::Uuid;

/// Shared webhook configuration. `None` when `MEET__WEBHOOK_URL` is unset.
#[derive(Clone, Debug)]
pub struct WebhookConfig {
    pub url:    Arc<str>,
    pub client: reqwest::Client,
}

impl WebhookConfig {
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("MEET__WEBHOOK_URL").ok().filter(|v| !v.is_empty())?;
        Some(Self {
            url:    Arc::from(url.as_str()),
            client: reqwest::Client::new(),
        })
    }
}

#[derive(Serialize)]
struct Payload<'a> {
    event:     &'a str,
    tenant_id: Uuid,
    meeting:   Value,
}

/// Dispatch a webhook event in the background.
/// Does nothing when `cfg` is `None`.
pub fn dispatch(cfg: Option<&WebhookConfig>, event: &'static str, tenant_id: Uuid, meeting: Value) {
    let Some(cfg) = cfg else { return };
    let url    = cfg.url.clone();
    let client = cfg.client.clone();
    tokio::spawn(async move {
        let body = serde_json::to_value(Payload { event, tenant_id, meeting }).unwrap_or_default();
        if let Err(e) = client.post(url.as_ref()).json(&body).send().await {
            warn!(error = %e, event, "webhook dispatch failed");
        }
    });
}

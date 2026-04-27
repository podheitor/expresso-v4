//! Fire-and-forget webhook dispatch for meeting lifecycle events.
//!
//! Set `MEET__WEBHOOK_URL` to receive POST callbacks on meeting events.
//! Payload: `{"event": "<kind>", "tenant_id": "...", "meeting": {...}}`.
//! Delivery uses exponential backoff (up to 3 attempts: 1s, 2s, 4s delays).
//! Failures after all retries are logged but never propagate to callers.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tracing::{info, warn};
use uuid::Uuid;

const MAX_ATTEMPTS: u32 = 3;

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

/// Dispatch a webhook event in the background with exponential backoff retry.
/// Does nothing when `cfg` is `None`.
pub fn dispatch(cfg: Option<&WebhookConfig>, event: &'static str, tenant_id: Uuid, meeting: Value) {
    let Some(cfg) = cfg else { return };
    let url    = cfg.url.clone();
    let client = cfg.client.clone();
    tokio::spawn(async move {
        let body = serde_json::to_value(Payload { event, tenant_id, meeting }).unwrap_or_default();
        let mut delay = Duration::from_secs(1);
        for attempt in 1..=MAX_ATTEMPTS {
            match client.post(url.as_ref()).json(&body).send().await {
                Ok(resp) if resp.status().is_success() => {
                    if attempt > 1 {
                        info!(event, attempt, "webhook delivered after retry");
                    }
                    return;
                }
                Ok(resp) => {
                    warn!(event, attempt, status = %resp.status(), "webhook non-2xx response");
                }
                Err(e) => {
                    warn!(error = %e, event, attempt, "webhook dispatch failed");
                }
            }
            if attempt < MAX_ATTEMPTS {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
        }
        warn!(event, "webhook delivery failed after {} attempts", MAX_ATTEMPTS);
    });
}

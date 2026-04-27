//! Shared state — DB pool + Jitsi JWT issuer + webhook config.

use std::sync::Arc;

use expresso_core::DbPool;

use crate::error::{MeetError, Result};
use crate::jitsi::Jitsi;
use crate::webhook::WebhookConfig;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    db:      Option<DbPool>,
    jitsi:   Option<Jitsi>,
    webhook: Option<WebhookConfig>,
}

impl AppState {
    pub fn new(db: Option<DbPool>, jitsi: Option<Jitsi>, webhook: Option<WebhookConfig>) -> Self {
        Self(Arc::new(Inner { db, jitsi, webhook }))
    }

    pub fn db(&self) -> Option<&DbPool> { self.0.db.as_ref() }

    pub fn db_or_unavailable(&self) -> Result<&DbPool> {
        self.0.db.as_ref().ok_or(MeetError::DatabaseUnavailable)
    }

    pub fn jitsi_or_unavailable(&self) -> Result<&Jitsi> {
        self.0.jitsi.as_ref().ok_or(MeetError::JitsiUnavailable)
    }

    pub fn webhook(&self) -> Option<&WebhookConfig> { self.0.webhook.as_ref() }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

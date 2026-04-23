//! Shared service state

use std::sync::Arc;

use expresso_auth_client::KcBasicAuthenticator;
use expresso_core::DbPool;

use crate::events::EventBus;

use crate::error::{CalendarError, Result};

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    db:         Option<DbPool>,
    kc_basic:   Option<KcBasicAuthenticator>,
    events:     EventBus,
}

impl AppState {
    pub fn new(db: Option<DbPool>, kc_basic: Option<KcBasicAuthenticator>, events: EventBus) -> Self {
        Self(Arc::new(Inner { db, kc_basic, events }))
    }

    pub fn events(&self) -> &EventBus {
        &self.0.events
    }

    pub fn db(&self) -> Option<&DbPool> {
        self.0.db.as_ref()
    }

    pub fn kc_basic(&self) -> Option<&KcBasicAuthenticator> {
        self.0.kc_basic.as_ref()
    }

    /// DB reference or SERVICE_UNAVAILABLE error — for handlers that require DB.
    pub fn db_or_unavailable(&self) -> Result<&DbPool> {
        self.0.db.as_ref().ok_or(CalendarError::DatabaseUnavailable)
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

//! Shared service state

use std::sync::Arc;

use expresso_auth_client::KcBasicAuthenticator;
use expresso_core::DbPool;

use crate::error::{ContactsError, Result};
use crate::events::ContactsEventBus;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    db: Option<DbPool>,
    kc_basic: Option<KcBasicAuthenticator>,
    bus: ContactsEventBus,
    search_url: String,
    search_token: String,
}

impl AppState {
    /// Construct with full-text search wired. Empty `search_url` disables
    /// indexing (the helpers no-op), keeping search an optional dependency.
    pub fn with_search(
        db: Option<DbPool>,
        kc_basic: Option<KcBasicAuthenticator>,
        bus: ContactsEventBus,
        search_url: String,
        search_token: String,
    ) -> Self {
        Self(Arc::new(Inner {
            db,
            kc_basic,
            bus,
            search_url,
            search_token,
        }))
    }

    pub fn bus(&self) -> &ContactsEventBus {
        &self.0.bus
    }

    pub fn search_url(&self) -> &str {
        &self.0.search_url
    }

    pub fn search_token(&self) -> &str {
        &self.0.search_token
    }

    pub fn db(&self) -> Option<&DbPool> {
        self.0.db.as_ref()
    }

    pub fn kc_basic(&self) -> Option<&KcBasicAuthenticator> {
        self.0.kc_basic.as_ref()
    }

    pub fn db_or_unavailable(&self) -> Result<&DbPool> {
        self.0.db.as_ref().ok_or(ContactsError::DatabaseUnavailable)
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

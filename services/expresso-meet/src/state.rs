//! Shared state — DB pool + Jitsi JWT issuer.

use std::sync::Arc;

use expresso_core::DbPool;

use crate::error::{MeetError, Result};
use crate::jitsi::Jitsi;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    db:     Option<DbPool>,
    jitsi:  Option<Jitsi>,
}

impl AppState {
    pub fn new(db: Option<DbPool>, jitsi: Option<Jitsi>) -> Self {
        Self(Arc::new(Inner { db, jitsi }))
    }

    pub fn db(&self) -> Option<&DbPool> { self.0.db.as_ref() }

    pub fn db_or_unavailable(&self) -> Result<&DbPool> {
        self.0.db.as_ref().ok_or(MeetError::DatabaseUnavailable)
    }

    pub fn jitsi_or_unavailable(&self) -> Result<&Jitsi> {
        self.0.jitsi.as_ref().ok_or(MeetError::JitsiUnavailable)
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

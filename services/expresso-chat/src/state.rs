//! Shared service state — DB pool + Matrix client.

use std::sync::Arc;

use expresso_core::DbPool;

use crate::error::{ChatError, Result};
use crate::events::ChatBus;
use crate::matrix::MatrixClient;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    db: Option<DbPool>,
    matrix: Option<MatrixClient>,
    bus: ChatBus,
}

impl AppState {
    pub fn new(db: Option<DbPool>, matrix: Option<MatrixClient>) -> Self {
        Self(Arc::new(Inner {
            db,
            matrix,
            bus: ChatBus::new(),
        }))
    }

    pub fn db(&self) -> Option<&DbPool> {
        self.0.db.as_ref()
    }

    pub fn db_or_unavailable(&self) -> Result<&DbPool> {
        self.0.db.as_ref().ok_or(ChatError::DatabaseUnavailable)
    }

    pub fn matrix_or_unavailable(&self) -> Result<&MatrixClient> {
        self.0.matrix.as_ref().ok_or(ChatError::MatrixUnavailable)
    }

    /// Process-local realtime bus (message/typing/presence → SSE).
    pub fn bus(&self) -> &ChatBus {
        &self.0.bus
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

//! Shared service state

use std::sync::Arc;

use expresso_core::DbPool;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    pub db: Option<DbPool>,
}

impl AppState {
    pub fn new(db: Option<DbPool>) -> Self {
        Self(Arc::new(Inner { db }))
    }

    pub fn db(&self) -> Option<&DbPool> {
        self.0.db.as_ref()
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

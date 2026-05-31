//! Shared service state — DB pool + search-index config.

use std::sync::Arc;

use expresso_core::DbPool;

use crate::error::{NotesError, Result};

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    db: Option<DbPool>,
    search_url: String,
    search_token: String,
}

impl AppState {
    pub fn new(db: Option<DbPool>, search_url: String, search_token: String) -> Self {
        Self(Arc::new(Inner {
            db,
            search_url,
            search_token,
        }))
    }

    pub fn db_or_unavailable(&self) -> Result<&DbPool> {
        self.0.db.as_ref().ok_or(NotesError::DatabaseUnavailable)
    }

    pub fn search_url(&self) -> &str {
        &self.0.search_url
    }

    pub fn search_token(&self) -> &str {
        &self.0.search_token
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

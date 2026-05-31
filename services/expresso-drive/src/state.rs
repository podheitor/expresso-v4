use std::{path::PathBuf, sync::Arc};

use expresso_core::DbPool;

use crate::error::{DriveError, Result};

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    db: Option<DbPool>,
    data_root: PathBuf,
    /// Base URL of expresso-search for full-text indexing (empty = disabled).
    search_url: String,
    /// Optional bearer token for the search service.
    search_token: String,
}

impl AppState {
    pub fn with_search(
        db: Option<DbPool>,
        data_root: PathBuf,
        search_url: String,
        search_token: String,
    ) -> Self {
        Self(Arc::new(Inner {
            db,
            data_root,
            search_url,
            search_token,
        }))
    }

    pub fn db_or_unavailable(&self) -> Result<&DbPool> {
        self.0.db.as_ref().ok_or(DriveError::DatabaseUnavailable)
    }

    pub fn data_root(&self) -> &PathBuf {
        &self.0.data_root
    }

    /// expresso-search base URL (empty when indexing is disabled).
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

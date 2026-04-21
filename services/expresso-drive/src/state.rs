use std::{path::PathBuf, sync::Arc};

use expresso_core::DbPool;

use crate::error::{DriveError, Result};

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    db:        Option<DbPool>,
    data_root: PathBuf,
}

impl AppState {
    pub fn new(db: Option<DbPool>, data_root: PathBuf) -> Self {
        Self(Arc::new(Inner { db, data_root }))
    }

    pub fn db_or_unavailable(&self) -> Result<&DbPool> {
        self.0.db.as_ref().ok_or(DriveError::DatabaseUnavailable)
    }

    pub fn data_root(&self) -> &PathBuf {
        &self.0.data_root
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

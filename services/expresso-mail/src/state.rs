//! Shared service state — cloned cheaply via Arc

use std::sync::Arc;
use expresso_core::{AppConfig, DbPool, RedisPool};

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

#[allow(dead_code)]
struct Inner {
    pub cfg: AppConfig,
    pub db:  DbPool,
    pub redis: RedisPool,
}

impl AppState {
    pub fn new(cfg: AppConfig, db: DbPool, redis: RedisPool) -> Self {
        Self(Arc::new(Inner { cfg, db, redis }))
    }

    pub fn cfg(&self)   -> &AppConfig  { &self.0.cfg }
    pub fn db(&self)    -> &DbPool     { &self.0.db }
    #[allow(dead_code)]
    pub fn redis(&self) -> &RedisPool  { &self.0.redis }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

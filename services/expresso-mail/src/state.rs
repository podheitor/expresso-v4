//! Shared service state — cloned cheaply via Arc

use std::sync::Arc;
use expresso_core::{AppConfig, DbPool, RedisPool};
use expresso_storage::ObjectStore;
use crate::dkim::DkimSignerState;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

#[allow(dead_code)]
struct Inner {
    pub cfg: AppConfig,
    pub db:  DbPool,
    pub redis: RedisPool,
    pub store: Option<ObjectStore>,
    pub dkim: Option<DkimSignerState>,
}

impl AppState {
    pub fn new(cfg: AppConfig, db: DbPool, redis: RedisPool) -> Self {
        Self(Arc::new(Inner { cfg, db, redis, store: None, dkim: None }))
    }

    pub fn with_store(cfg: AppConfig, db: DbPool, redis: RedisPool, store: ObjectStore) -> Self {
        Self(Arc::new(Inner { cfg, db, redis, store: Some(store), dkim: None }))
    }

    pub fn set_dkim(mut self, signer: DkimSignerState) -> Self {
        let inner = Arc::get_mut(&mut self.0).expect("set_dkim must be called before cloning");
        inner.dkim = Some(signer);
        self
    }

    pub fn cfg(&self)   -> &AppConfig       { &self.0.cfg }
    pub fn db(&self)    -> &DbPool          { &self.0.db }
    #[allow(dead_code)]
    pub fn redis(&self) -> &RedisPool       { &self.0.redis }
    pub fn store(&self) -> Option<&ObjectStore> { self.0.store.as_ref() }
    pub fn dkim(&self)  -> Option<&DkimSignerState> { self.0.dkim.as_ref() }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

//! App state + pending-login store.
//!
//! Pending logins: keyed by CSRF `state`, hold PKCE verifier until callback.
//! In-memory + TTL. Production: replace with Redis/signed-cookie for
//! multi-instance deployments.

use std::{collections::HashMap, sync::Arc, time::{Duration, Instant}};

use tokio::sync::Mutex;

use expresso_auth_client::OidcValidator;

use crate::config::RpConfig;
use crate::oidc::discovery::ProviderMetadata;

#[derive(Debug, Clone)]
pub struct PendingLogin {
    pub code_verifier:        String,
    pub post_login_redirect:  Option<String>,
    pub expires_at:           Instant,
}

pub struct AppState {
    pub cfg:       RpConfig,
    pub provider:  ProviderMetadata,
    pub http:      reqwest::Client,
    pub validator: Arc<OidcValidator>,
    pub pending:   Mutex<HashMap<String, PendingLogin>>,
    /// Optional DB pool for audit writes. None ⇒ audit disabled (service still serves OIDC).
    pub pool:      Option<sqlx::PgPool>,
}

impl AppState {
    pub async fn insert_pending(&self, state: String, pl: PendingLogin) {
        let mut m = self.pending.lock().await;
        Self::evict_expired(&mut m);
        m.insert(state, pl);
    }

    pub async fn take_pending(&self, state: &str) -> Option<PendingLogin> {
        let mut m = self.pending.lock().await;
        Self::evict_expired(&mut m);
        m.remove(state)
    }

    fn evict_expired(m: &mut HashMap<String, PendingLogin>) {
        let now = Instant::now();
        m.retain(|_, v| v.expires_at > now);
    }

    pub fn state_ttl(&self) -> Duration { self.cfg.state_ttl }
}

//! App state + pending-login store.
//!
//! Pending logins: keyed by CSRF `state`, hold PKCE verifier + resolved
//! redirect_uri + optional realm (multi-tenant). In-memory + TTL.
//! Production multi-instance: replace with Redis/signed-cookie.

use std::{collections::HashMap, sync::Arc, time::{Duration, Instant}};

use tokio::sync::Mutex;

use expresso_auth_client::{MultiRealmValidator, OidcValidator, TenantResolver};

use crate::config::RpConfig;
use crate::oidc::discovery::ProviderMetadata;
use crate::oidc::multi_provider::TenantProviderCache;

#[derive(Debug, Clone)]
pub struct PendingLogin {
    pub code_verifier:        String,
    pub post_login_redirect:  Option<String>,
    pub redirect_uri:         String,
    pub realm:                Option<String>,
    pub expires_at:           Instant,
}

pub struct AppState {
    pub cfg:             RpConfig,
    pub provider:        ProviderMetadata,
    pub http:            reqwest::Client,
    pub validator:       Arc<OidcValidator>,
    pub multi_validator: Option<Arc<MultiRealmValidator>>,
    pub tenant_resolver: Option<Arc<TenantResolver>>,
    pub multi_provider:  Option<Arc<TenantProviderCache>>,
    pub pending:         Mutex<HashMap<String, PendingLogin>>,
    pub pool:            Option<sqlx::PgPool>,
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

    /// Multi-tenant mode = resolver + provider cache both present.
    pub fn is_multi(&self) -> bool {
        self.tenant_resolver.is_some() && self.multi_provider.is_some()
    }

    /// Resolve realm from Host header when in multi-tenant mode.
    /// Returns `None` when single-realm or host unknown.
    pub fn realm_for_host(&self, host: &str) -> Option<String> {
        self.tenant_resolver.as_ref()?.resolve(host).map(str::to_string)
    }

    /// Build tenant-scoped redirect_uri from Host. Falls back to static cfg.
    pub fn redirect_uri_for_host(&self, host: &str) -> String {
        self.cfg.redirect_uri_template.as_deref()
            .map(|t| t.replace("{host}", host))
            .unwrap_or_else(|| self.cfg.redirect_uri.clone())
    }

    pub fn post_logout_for_host(&self, host: &str) -> Option<String> {
        self.cfg.post_logout_template.as_deref()
            .map(|t| t.replace("{host}", host))
            .or_else(|| self.cfg.post_logout_redirect_uri.clone())
    }
}

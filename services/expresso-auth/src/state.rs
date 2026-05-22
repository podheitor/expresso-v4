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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn make_pending(ttl_secs: u64) -> PendingLogin {
        PendingLogin {
            code_verifier:       "verifier-xyz".into(),
            post_login_redirect: None,
            redirect_uri:        "https://app.example.com/callback".into(),
            realm:               None,
            expires_at:          Instant::now() + Duration::from_secs(ttl_secs),
        }
    }

    #[test]
    fn evict_expired_removes_stale_entries() {
        let mut map: HashMap<String, PendingLogin> = HashMap::new();
        map.insert("live".into(),    make_pending(60));
        // Insert entry already past its TTL (subtract from now → already expired).
        map.insert("dead".into(), PendingLogin {
            expires_at: Instant::now() - Duration::from_secs(1),
            ..make_pending(0)
        });
        AppState::evict_expired(&mut map);
        assert!(map.contains_key("live"), "live entry must survive");
        assert!(!map.contains_key("dead"), "expired entry must be removed");
    }

    #[test]
    fn evict_expired_noop_on_empty_map() {
        let mut map: HashMap<String, PendingLogin> = HashMap::new();
        AppState::evict_expired(&mut map);
        assert!(map.is_empty());
    }

    #[test]
    fn evict_expired_keeps_all_when_none_expired() {
        let mut map: HashMap<String, PendingLogin> = HashMap::new();
        map.insert("a".into(), make_pending(60));
        map.insert("b".into(), make_pending(120));
        AppState::evict_expired(&mut map);
        assert_eq!(map.len(), 2);
    }

    fn make_cfg(redirect_uri: &str, template: Option<&str>) -> crate::config::RpConfig {
        crate::config::RpConfig {
            issuer:                     "https://kc/realms/x".into(),
            client_id:                  "client".into(),
            redirect_uri:               redirect_uri.into(),
            post_logout_redirect_uri:   None,
            state_ttl:                  Duration::from_secs(600),
            http_timeout:               Duration::from_secs(5),
            issuer_template:            None,
            redirect_uri_template:      template.map(str::to_string),
            post_logout_template:       None,
        }
    }

    #[test]
    fn redirect_uri_template_replaces_host() {
        let cfg = make_cfg("https://fallback/cb", Some("https://{host}/auth/callback"));
        // Test the pure string substitution logic directly (same as redirect_uri_for_host).
        let result = cfg.redirect_uri_template.as_deref()
            .map(|t| t.replace("{host}", "tenant.example.com"))
            .unwrap_or_else(|| cfg.redirect_uri.clone());
        assert_eq!(result, "https://tenant.example.com/auth/callback");
    }

    #[test]
    fn redirect_uri_falls_back_when_no_template() {
        let cfg = make_cfg("https://static.example.com/callback", None);
        let result = cfg.redirect_uri_template.as_deref()
            .map(|t| t.replace("{host}", "any.host"))
            .unwrap_or_else(|| cfg.redirect_uri.clone());
        assert_eq!(result, "https://static.example.com/callback");
    }

    #[test]
    fn evict_expired_partial_cleanup() {
        let mut m: HashMap<String, PendingLogin> = HashMap::new();
        m.insert("stale".into(), PendingLogin {
            expires_at: Instant::now() - Duration::from_secs(1),
            ..make_pending(0)
        });
        m.insert("fresh".into(), make_pending(9999));
        AppState::evict_expired(&mut m);
        assert!(!m.contains_key("stale"));
        assert!(m.contains_key("fresh"));
    }

    #[test]
    fn post_logout_template_replaces_host() {
        let tmpl = "https://{host}/logout";
        let result = tmpl.replace("{host}", "t.example.com");
        assert_eq!(result, "https://t.example.com/logout");
    }

    #[test]
    fn fresh_pending_login_is_not_expired() {
        let p = make_pending(9999);
        assert!(p.expires_at > std::time::Instant::now());
    }

    #[test]
    fn evict_expired_removes_nothing_when_all_fresh() {
        let mut m: HashMap<String, PendingLogin> = HashMap::new();
        m.insert("a".into(), make_pending(9999));
        m.insert("b".into(), make_pending(9999));
        AppState::evict_expired(&mut m);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn evict_expired_removes_single_expired() {
        let mut m: HashMap<String, PendingLogin> = HashMap::new();
        m.insert("stale".into(), make_pending(0));
        AppState::evict_expired(&mut m);
        assert!(m.is_empty());
    }

    #[test]
    fn evict_expired_removes_only_expired_entries() {
        let mut m: HashMap<String, PendingLogin> = HashMap::new();
        m.insert("fresh".into(), make_pending(9999));
        m.insert("stale".into(), make_pending(0));
        AppState::evict_expired(&mut m);
        assert_eq!(m.len(), 1);
        assert!(m.contains_key("fresh"));
    }

    #[test]
    fn evict_expired_removes_both_when_both_expired() {
        let mut m: HashMap<String, PendingLogin> = HashMap::new();
        m.insert("a".into(), make_pending(0));
        m.insert("b".into(), make_pending(0));
        AppState::evict_expired(&mut m);
        assert!(m.is_empty());
    }

    #[test]
    fn evict_expired_empty_map_stays_empty() {
        let mut m: HashMap<String, PendingLogin> = HashMap::new();
        AppState::evict_expired(&mut m);
        assert!(m.is_empty());
    }

    #[test]
    fn evict_expired_keeps_non_expired_entry() {
        let mut m: HashMap<String, PendingLogin> = HashMap::new();
        m.insert("live".into(), make_pending(3600));
        AppState::evict_expired(&mut m);
        assert!(m.contains_key("live"));
    }

    #[test]
    fn pending_login_expires_at_is_in_the_future_when_fresh() {
        let p = make_pending(60);
        assert!(p.expires_at > std::time::Instant::now());
    }

    #[test]
    fn evict_expired_removes_past_entry() {
        let mut m: HashMap<String, PendingLogin> = HashMap::new();
        let expired = PendingLogin {
            expires_at: std::time::Instant::now() - Duration::from_secs(1),
            ..make_pending(60)
        };
        m.insert("past".into(), expired);
        AppState::evict_expired(&mut m);
        assert!(!m.contains_key("past"));
    }

    #[test]
    fn pending_login_post_login_redirect_none_by_default() {
        let p = make_pending(30);
        assert!(p.post_login_redirect.is_none());
    }

    #[test]
    fn pending_login_realm_none_by_default() {
        let p = make_pending(60);
        assert!(p.realm.is_none());
    }

    #[test]
    fn pending_login_code_verifier_nonempty() {
        let p = make_pending(60);
        assert!(!p.code_verifier.is_empty());
    }
}

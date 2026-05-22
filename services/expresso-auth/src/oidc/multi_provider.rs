//! Per-tenant OIDC provider metadata cache.
//!
//! Lazy-fetches `{issuer-template-with-realm}/.well-known/openid-configuration`
//! once per realm + caches. Used by multi-realm login/callback/logout flow.

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::sync::RwLock;

use crate::error::{Result, RpError};
use crate::oidc::discovery::ProviderMetadata;

/// Template placeholder replaced with tenant realm.
const REALM_PLACEHOLDER: &str = "{realm}";

/// Multi-tenant provider metadata cache.
pub struct TenantProviderCache {
    template: String,
    timeout:  Duration,
    cache:    RwLock<HashMap<String, Arc<ProviderMetadata>>>,
}

impl TenantProviderCache {
    pub fn new(template: String, timeout: Duration) -> Result<Self> {
        if !template.contains(REALM_PLACEHOLDER) {
            return Err(RpError::Config(format!(
                "AUTH_RP__ISSUER_TEMPLATE missing '{}' placeholder", REALM_PLACEHOLDER
            )));
        }
        Ok(Self { template, timeout, cache: RwLock::new(HashMap::new()) })
    }

    /// Return cached provider or fetch+cache on miss.
    pub async fn get_or_fetch(&self, realm: &str) -> Result<Arc<ProviderMetadata>> {
        if let Some(p) = self.cache.read().await.get(realm).cloned() {
            return Ok(p);
        }
        let issuer = self.template.replace(REALM_PLACEHOLDER, realm);
        let md = ProviderMetadata::fetch(&issuer, self.timeout).await?;
        let arc = Arc::new(md);
        self.cache.write().await.insert(realm.to_string(), arc.clone());
        Ok(arc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_template_without_placeholder() {
        let r = TenantProviderCache::new("http://kc/realms/x".into(), Duration::from_secs(1));
        assert!(r.is_err());
    }

    #[test]
    fn accepts_template_with_placeholder() {
        let r = TenantProviderCache::new("http://kc/realms/{realm}".into(), Duration::from_secs(1));
        assert!(r.is_ok());
    }

    #[test]
    fn rejects_empty_template() {
        let r = TenantProviderCache::new("".into(), Duration::from_secs(1));
        assert!(r.is_err());
    }

    #[test]
    fn realm_placeholder_constant() {
        assert_eq!(REALM_PLACEHOLDER, "{realm}");
    }

    #[test]
    fn placeholder_substitution_in_template() {
        let tmpl = "http://kc/realms/{realm}/protocol/openid-connect";
        let issuer = tmpl.replace(REALM_PLACEHOLDER, "expresso");
        assert_eq!(issuer, "http://kc/realms/expresso/protocol/openid-connect");
    }

    #[test]
    fn rejects_template_with_wrong_placeholder() {
        let r = TenantProviderCache::new("http://kc/realms/{tenant}".into(), Duration::from_secs(1));
        assert!(r.is_err());
    }

    #[test]
    fn realm_placeholder_is_realm_keyword() {
        assert_eq!(REALM_PLACEHOLDER, "{realm}");
        assert!(REALM_PLACEHOLDER.contains("realm"));
    }

    #[test]
    fn valid_template_accepted() {
        let r = TenantProviderCache::new("https://auth.svc/realms/{realm}".into(), Duration::from_secs(60));
        assert!(r.is_ok());
    }
}

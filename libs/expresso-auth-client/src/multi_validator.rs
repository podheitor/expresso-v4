//! Multi-realm OIDC validator — lazily builds `OidcValidator` per realm.
//!
//! Fase 2 do realm-per-tenant: cada tenant valida tokens contra SEU próprio
//! realm Keycloak. Issuer template substitui `{realm}` com o nome do realm.
//!
//! Exemplo:
//!   template  = "http://keycloak:8080/realms/{realm}"
//!   realm     = "acme"
//!   → issuer = "http://keycloak:8080/realms/acme"
//!
//! Cache: once built, validators persist forever (JWKS refresh é interno ao
//! OidcValidator). Racing builders para o mesmo realm resolvem via Mutex +
//! double-check.

use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::error::{AuthError, Result};
use crate::validator::{OidcConfig, OidcValidator};


pub struct MultiRealmValidator {
    issuer_template: String,
    audience:        String,
    cache: RwLock<HashMap<String, Arc<OidcValidator>>>,
}

impl MultiRealmValidator {
    /// `issuer_template` DEVE conter o literal `{realm}`; será substituído
    /// pelo nome do realm a cada build.
    pub fn new(issuer_template: impl Into<String>, audience: impl Into<String>) -> Result<Self> {
        let tpl = issuer_template.into();
        if !tpl.contains("{realm}") {
            return Err(AuthError::Config(
                "issuer_template must contain '{realm}' placeholder".into()
            ));
        }
        Ok(Self {
            issuer_template: tpl,
            audience:        audience.into(),
            cache:           RwLock::new(HashMap::new()),
        })
    }

    /// Returns validator for `realm`, building lazily on first access.
    pub async fn for_realm(&self, realm: &str) -> Result<Arc<OidcValidator>> {
        if let Some(v) = self.cache.read().await.get(realm).cloned() {
            return Ok(v);
        }
        // Double-check under write lock — another task may have built it.
        let mut w = self.cache.write().await;
        if let Some(v) = w.get(realm).cloned() {
            return Ok(v);
        }
        let issuer = self.issuer_for(realm);
        debug!(%realm, %issuer, "building validator for realm");
        let cfg = OidcConfig::new(issuer.clone(), self.audience.clone());
        let v   = Arc::new(OidcValidator::new(cfg).await?);
        w.insert(realm.to_string(), v.clone());
        crate::metrics::REALM_CACHE_SIZE.set(w.len() as i64);
        info!(%realm, validators_cached = w.len(), "realm validator ready");
        Ok(v)
    }

    /// Substitute `{realm}` in the template.
    pub fn issuer_for(&self, realm: &str) -> String {
        self.issuer_template.replace("{realm}", realm)
    }

    pub fn audience(&self) -> &str { &self.audience }

    /// Number of realms currently cached.
    pub async fn cached_realms(&self) -> usize {
        self.cache.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_template_without_placeholder() {
        let r = MultiRealmValidator::new("http://kc/realms/fixed", "aud");
        assert!(matches!(r, Err(AuthError::Config(_))));
    }

    #[test]
    fn accepts_template_with_placeholder() {
        let m = MultiRealmValidator::new("http://kc:8080/realms/{realm}", "expresso-web").unwrap();
        assert_eq!(m.audience(), "expresso-web");
        assert_eq!(m.issuer_for("acme"), "http://kc:8080/realms/acme");
        assert_eq!(m.issuer_for("demo"), "http://kc:8080/realms/demo");
    }

    #[test]
    fn substitutes_placeholder_once() {
        let m = MultiRealmValidator::new("https://auth.ex/realms/{realm}/", "web").unwrap();
        assert_eq!(m.issuer_for("t-42"), "https://auth.ex/realms/t-42/");
    }
}

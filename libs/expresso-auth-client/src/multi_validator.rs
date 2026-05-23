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

    #[test]
    fn audience_accessor() {
        let m = MultiRealmValidator::new("http://kc/realms/{realm}", "my-client").unwrap();
        assert_eq!(m.audience(), "my-client");
    }

    #[test]
    fn issuer_for_empty_realm() {
        let m = MultiRealmValidator::new("http://kc/realms/{realm}", "aud").unwrap();
        assert_eq!(m.issuer_for(""), "http://kc/realms/");
    }

    #[test]
    fn template_with_multiple_placeholder_occurrences() {
        let m = MultiRealmValidator::new("http://{realm}.kc/realms/{realm}", "aud").unwrap();
        // Rust's str::replace replaces all occurrences
        assert_eq!(m.issuer_for("demo"), "http://demo.kc/realms/demo");
    }

    #[test]
    fn issuer_for_hyphenated_realm() {
        let m = MultiRealmValidator::new("http://kc/realms/{realm}", "aud").unwrap();
        assert_eq!(m.issuer_for("tenant-42"), "http://kc/realms/tenant-42");
    }

    #[test]
    fn config_error_variant_on_missing_placeholder() {
        let err = MultiRealmValidator::new("http://kc/realms/static", "aud").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("{realm}") || matches!(err, AuthError::Config(_)));
    }

    #[test]
    fn issuer_for_numeric_realm() {
        let m = MultiRealmValidator::new("http://kc/realms/{realm}", "aud").unwrap();
        assert_eq!(m.issuer_for("12345"), "http://kc/realms/12345");
    }

    #[test]
    fn missing_placeholder_returns_error() {
        assert!(MultiRealmValidator::new("http://kc/realms/fixed", "aud").is_err());
    }

    #[test]
    fn issuer_for_replaces_placeholder_exactly_once() {
        let m = MultiRealmValidator::new("http://kc/realms/{realm}", "aud").unwrap();
        assert_eq!(m.issuer_for("acme"), "http://kc/realms/acme");
    }

    #[test]
    fn issuer_for_different_realms_differ() {
        let m = MultiRealmValidator::new("http://kc/realms/{realm}", "aud").unwrap();
        assert_ne!(m.issuer_for("corp"), m.issuer_for("demo"));
    }

    #[test]
    fn issuer_for_contains_realm_name() {
        let m = MultiRealmValidator::new("http://kc/realms/{realm}", "aud").unwrap();
        assert!(m.issuer_for("mytenant").contains("mytenant"));
    }

    #[test]
    fn issuer_for_replaces_placeholder_with_realm() {
        let m = MultiRealmValidator::new("http://kc/realms/{realm}/oidc", "aud").unwrap();
        assert_eq!(m.issuer_for("test"), "http://kc/realms/test/oidc");
    }

    #[test]
    fn audience_accessor_returns_configured_aud() {
        let m = MultiRealmValidator::new("http://kc/realms/{realm}", "my-service").unwrap();
        assert_eq!(m.audience(), "my-service");
    }

    #[test]
    fn issuer_template_stores_placeholder_verbatim() {
        let m = MultiRealmValidator::new("https://idp/{realm}/oidc", "svc").unwrap();
        assert!(m.issuer_template.contains("{realm}"));
    }

    #[test]
    fn missing_placeholder_returns_config_error() {
        let err = MultiRealmValidator::new("https://idp/oidc", "svc").unwrap_err();
        assert!(matches!(err, crate::error::AuthError::Config(_)));
    }

    #[test]
    fn audience_is_stored_verbatim() {
        let m = MultiRealmValidator::new("http://kc/realms/{realm}", "expresso-dav").unwrap();
        assert_eq!(m.audience(), "expresso-dav");
    }

    #[test]
    fn issuer_for_special_chars_realm_includes_realm() {
        let m = MultiRealmValidator::new("http://kc/realms/{realm}", "aud").unwrap();
        let iss = m.issuer_for("my-realm_01");
        assert!(iss.contains("my-realm_01"));
    }

    #[test]
    fn issuer_template_field_contains_placeholder_after_construction() {
        let m = MultiRealmValidator::new("https://sso/{realm}/token", "client").unwrap();
        assert!(m.issuer_template.contains("{realm}"));
        assert!(!m.issuer_for("acme").contains("{realm}"));
    }

    #[test]
    fn issuer_for_uuid_realm_produces_valid_url_segment() {
        let m = MultiRealmValidator::new("https://kc/realms/{realm}", "aud").unwrap();
        let realm = "a1b2c3d4-0000-0000-0000-000000000001";
        let iss = m.issuer_for(realm);
        assert!(iss.ends_with(realm));
    }

    #[test]
    fn issuer_for_does_not_contain_placeholder_after_substitution() {
        let m = MultiRealmValidator::new("https://kc/realms/{realm}", "aud").unwrap();
        let iss = m.issuer_for("myrealm");
        assert!(!iss.contains("{realm}"));
    }

    #[test]
    fn issuer_for_long_realm_name_is_fully_substituted() {
        let m = MultiRealmValidator::new("https://kc/realms/{realm}", "aud").unwrap();
        let realm = "a-very-long-tenant-realm-name-0001";
        let iss = m.issuer_for(realm);
        assert!(iss.ends_with(realm));
        assert!(!iss.contains("{realm}"));
    }
}

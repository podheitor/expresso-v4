//! Raw JWT claims → normalized `AuthContext`.
//!
//! Keycloak emits tenancy metadata via a custom claim (`tenant_id`). Role
//! information lives in `realm_access.roles` + `resource_access.<aud>.roles`.

use std::collections::HashMap;

use serde::Deserialize;
use uuid::Uuid;

use crate::error::{AuthError, Result};


/// Extract realm name from a Keycloak `iss` claim of the form
/// `https://host[:port]/realms/<realm>` (with optional trailing slash).
/// Returns `None` if the string does not contain `/realms/`.
pub(crate) fn realm_from_iss(iss: &str) -> Option<&str> {
    let (_, tail) = iss.rsplit_once("/realms/")?;
    let realm = tail.split('/').next()?;
    if realm.is_empty() { None } else { Some(realm) }
}

/// Raw OIDC claims as emitted by Keycloak. Extra fields are preserved in
/// `extra` for downstream inspection without changing this struct.
#[derive(Debug, Clone, Deserialize)]
pub struct RawClaims {
    pub sub:       String,
    pub iss:       String,
    #[serde(default)]
    pub aud:       AudClaim,
    pub exp:       i64,
    #[serde(default)]
    pub email:     Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub name:      Option<String>,
    #[serde(default, rename = "tenant_id")]
    pub tenant_id: Option<String>,
    #[serde(default)]
    pub realm_access: Option<RolesBlock>,
    #[serde(default)]
    pub resource_access: HashMap<String, RolesBlock>,
    /// Authentication Context Class Reference (OIDC §2). e.g. "1", "urn:govbr:loa:ouro".
    #[serde(default)]
    pub acr:       Option<String>,
    /// Authentication Methods References (RFC 8176). e.g. ["pwd","otp"], ["pwd","hwk"].
    #[serde(default)]
    pub amr:       Option<Vec<String>>,
    /// gov.br federated identity — CPF hash (set by KC IdP mapper).
    #[serde(default)]
    pub govbr_cpf_hash: Option<String>,
    /// gov.br "Selos de Confiabilidade" list (e.g. ["cadastro-basico","biometria"]).
    #[serde(default)]
    pub govbr_confiabilidades: Option<Vec<String>>,
}

/// `aud` can be a single string or an array per RFC 7519 §4.1.3.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(untagged)]
pub enum AudClaim {
    #[default]
    Empty,
    One(String),
    Many(Vec<String>),
}

impl AudClaim {
    pub fn contains(&self, needle: &str) -> bool {
        match self {
            Self::Empty   => false,
            Self::One(v)  => v == needle,
            Self::Many(v) => v.iter().any(|s| s == needle),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RolesBlock {
    #[serde(default)]
    pub roles: Vec<String>,
}

/// Normalized authentication context exposed to services.
///
/// Invariants:
/// - `user_id` + `tenant_id` are parsed UUIDs (claims are strings on the wire).
/// - `roles` merges `realm_access.roles` with `resource_access.<aud>.roles` for
///   the primary audience — callers see a single flat role set.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user_id:      Uuid,
    pub tenant_id:    Uuid,
    pub email:        String,
    pub display_name: String,
    pub roles:        Vec<String>,
    pub expires_at:   i64,
    /// Raw ACR string from IdP (OIDC §2).
    pub acr:          Option<String>,
    /// Raw AMR list from IdP (RFC 8176).
    pub amr:          Vec<String>,
    /// gov.br CPF hash when federated via gov.br IdP.
    pub govbr_cpf_hash: Option<String>,
    /// gov.br confiabilidades list (empty when not federated via gov.br).
    pub govbr_confiabilidades: Vec<String>,
}

impl AuthContext {
    pub fn has_role(&self, needle: &str) -> bool {
        self.roles.iter().any(|r| r == needle)
    }

    /// True if `self.roles` contains at least one of `needles`.
    pub fn has_any_role(&self, needles: &[&str]) -> bool {
        needles.iter().any(|n| self.has_role(n))
    }

    /// Build an `AuthContext` from raw claims. `primary_audience` is the
    /// client_id registered in Keycloak — used to pick the right
    /// `resource_access.<aud>.roles` bucket.
    pub fn from_raw(raw: RawClaims, primary_audience: &str) -> Result<Self> {
        let user_id = Uuid::parse_str(&raw.sub)
            .map_err(|e| AuthError::MalformedClaim("sub", e.to_string()))?;
        // Tenant derivation: prefer realm name from `iss` (realm-per-tenant model);
        // fall back to legacy `tenant_id` claim for tokens emitted by single-realm
        // deployments that still carry the hardcoded-claim mapper.
        let tenant_id = realm_from_iss(&raw.iss)
            .and_then(|r| Uuid::parse_str(r.trim()).ok())
            .or_else(|| raw.tenant_id.as_deref().and_then(|s| Uuid::parse_str(s.trim()).ok()))
            .ok_or(AuthError::MissingClaim("tenant_id"))?;

        let email = raw.email.clone().unwrap_or_default();
        let display_name = raw.name
            .or(raw.preferred_username.clone())
            .unwrap_or_else(|| format!("user-{}", &user_id.to_string()[..8]));

        let mut roles: Vec<String> = raw.realm_access
            .map(|b| b.roles)
            .unwrap_or_default();
        if let Some(block) = raw.resource_access.get(primary_audience) {
            for r in &block.roles {
                if !roles.iter().any(|x| x == r) { roles.push(r.clone()); }
            }
        }

        Ok(Self {
            user_id, tenant_id, email, display_name,
            roles, expires_at: raw.exp,
            acr: raw.acr,
            amr: raw.amr.unwrap_or_default(),
            govbr_cpf_hash: raw.govbr_cpf_hash,
            govbr_confiabilidades: raw.govbr_confiabilidades.unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(roles: &[&str]) -> AuthContext {
        AuthContext {
            user_id:      Uuid::nil(),
            tenant_id:    Uuid::nil(),
            email:        "x@y".into(),
            display_name: "x".into(),
            roles:        roles.iter().map(|r| r.to_string()).collect(),
            expires_at:   0,
            acr:          None,
            amr:          Vec::new(),
            govbr_cpf_hash: None,
            govbr_confiabilidades: Vec::new(),
        }
    }

    #[test]
    fn has_role_matches_exact() {
        let c = ctx(&["admin", "user"]);
        assert!(c.has_role("admin"));
        assert!(c.has_role("user"));
        assert!(!c.has_role("Admin"));
        assert!(!c.has_role("missing"));
    }

    #[test]
    fn has_any_role_matches_one_of() {
        let c = ctx(&["user"]);
        assert!(c.has_any_role(&["admin", "user"]));
        assert!(c.has_any_role(&["user"]));
        assert!(!c.has_any_role(&["admin", "super"]));
        assert!(!c.has_any_role(&[]));
    }

    #[test]
    fn has_any_role_empty_roles_never_matches() {
        let c = ctx(&[]);
        assert!(!c.has_any_role(&["admin"]));
        assert!(!c.has_role("admin"));
    }

    #[test]
    fn realm_from_iss_extracts_last_segment() {
        assert_eq!(super::realm_from_iss("https://kc/realms/acme"), Some("acme"));
        assert_eq!(super::realm_from_iss("https://kc:8443/realms/acme/"), Some("acme"));
        assert_eq!(super::realm_from_iss("http://kc:8080/realms/acme/protocol/openid-connect"), Some("acme"));
        assert_eq!(super::realm_from_iss("https://kc/auth/realms/acme"), Some("acme"));
    }

    #[test]
    fn realm_from_iss_rejects_missing_segment() {
        assert_eq!(super::realm_from_iss("https://kc/"), None);
        assert_eq!(super::realm_from_iss(""), None);
        assert_eq!(super::realm_from_iss("https://kc/realms/"), None);
    }

    #[test]
    fn from_raw_derives_tenant_from_iss() {
        let uid = Uuid::new_v4();
        let tid = Uuid::new_v4();
        let raw = RawClaims {
            sub: uid.to_string(),
            iss: format!("https://kc/realms/{tid}"),
            aud: AudClaim::One("expresso-web".into()),
            exp: 0,
            email: None, preferred_username: None, name: None,
            tenant_id: None,
            realm_access: None,
            resource_access: HashMap::new(),
            acr: None, amr: None,
            govbr_cpf_hash: None, govbr_confiabilidades: None,
        };
        let c = AuthContext::from_raw(raw, "expresso-web").unwrap();
        assert_eq!(c.tenant_id, tid);
    }

    #[test]
    fn from_raw_falls_back_to_legacy_claim() {
        let uid = Uuid::new_v4();
        let tid = Uuid::new_v4();
        let raw = RawClaims {
            sub: uid.to_string(),
            iss: "https://kc/not-a-realm-url".into(),
            aud: AudClaim::One("expresso-web".into()),
            exp: 0,
            email: None, preferred_username: None, name: None,
            tenant_id: Some(tid.to_string()),
            realm_access: None,
            resource_access: HashMap::new(),
            acr: None, amr: None,
            govbr_cpf_hash: None, govbr_confiabilidades: None,
        };
        let c = AuthContext::from_raw(raw, "expresso-web").unwrap();
        assert_eq!(c.tenant_id, tid);
    }

    #[test]
    fn from_raw_errors_when_no_tenant_source() {
        let uid = Uuid::new_v4();
        let raw = RawClaims {
            sub: uid.to_string(),
            iss: "https://kc/realms/not-a-uuid".into(),
            aud: AudClaim::Empty,
            exp: 0,
            email: None, preferred_username: None, name: None,
            tenant_id: None,
            realm_access: None,
            resource_access: HashMap::new(),
            acr: None, amr: None,
            govbr_cpf_hash: None, govbr_confiabilidades: None,
        };
        let r = AuthContext::from_raw(raw, "expresso-web");
        assert!(matches!(r, Err(AuthError::MissingClaim("tenant_id"))));
    }
}

//! Raw JWT claims → normalized `AuthContext`.
//!
//! Keycloak emits tenancy metadata via a custom claim (`tenant_id`). Role
//! information lives in `realm_access.roles` + `resource_access.<aud>.roles`.

use std::collections::HashMap;

use serde::Deserialize;
use uuid::Uuid;

use crate::error::{AuthError, Result};

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
        let tenant_raw = raw.tenant_id
            .ok_or(AuthError::MissingClaim("tenant_id"))?;
        let tenant_id = Uuid::parse_str(tenant_raw.trim())
            .map_err(|e| AuthError::MalformedClaim("tenant_id", e.to_string()))?;

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
}

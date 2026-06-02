//! SAML federation detection from an `AuthContext`.
//!
//! A login brokered through a Keycloak SAML IdP carries the broker alias in the
//! `identity_provider` claim (stamped by the usersessionmodel-note mapper added
//! in seed-realm.sh §12). This module turns that claim into a typed marker the
//! callback handler uses to drive JIT provisioning. Mirrors `govbr.rs`.

use expresso_auth_client::AuthContext;

/// Metadata about a SAML-federated identity extracted from an `AuthContext`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamlFederation {
    /// Keycloak broker alias (= the `saml_idp_config.alias` row + kc_idp_hint).
    pub idp_alias: String,
    /// The IdP-asserted subject — Keycloak's `sub` for the brokered user, used
    /// as the stable key in `saml_user_map`.
    pub subject: String,
    /// Email mapped from the assertion (may be empty if the IdP omitted it).
    pub email: String,
    /// Display name mapped from the assertion (may be empty).
    pub display_name: String,
}

impl SamlFederation {
    /// Returns `Some` only when the token carries an `identity_provider` claim
    /// (i.e. the login was brokered through an external IdP). gov.br logins also
    /// set `identity_provider` (alias `govbr`); the caller filters those out by
    /// checking `govbr_cpf_hash` first, so SAML JIT only runs for true SAML
    /// brokers.
    pub fn from_ctx(ctx: &AuthContext) -> Option<Self> {
        let idp_alias = ctx.identity_provider.clone()?;
        if idp_alias.trim().is_empty() {
            return None;
        }
        Some(Self {
            idp_alias,
            subject: ctx.user_id.to_string(),
            email: ctx.email.clone(),
            display_name: ctx.display_name.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn ctx(idp: Option<&str>) -> AuthContext {
        AuthContext {
            user_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            email: "u@corp.com".into(),
            display_name: "User".into(),
            roles: vec![],
            expires_at: 0,
            acr: None,
            amr: vec![],
            govbr_cpf_hash: None,
            govbr_confiabilidades: vec![],
            identity_provider: idp.map(String::from),
            ldap_id: None,
        }
    }

    #[test]
    fn some_when_idp_present() {
        let f = SamlFederation::from_ctx(&ctx(Some("okta"))).unwrap();
        assert_eq!(f.idp_alias, "okta");
        assert_eq!(f.email, "u@corp.com");
        assert_eq!(f.display_name, "User");
        assert_eq!(f.subject, Uuid::nil().to_string());
    }

    #[test]
    fn none_when_idp_absent() {
        assert!(SamlFederation::from_ctx(&ctx(None)).is_none());
    }

    #[test]
    fn none_when_idp_blank() {
        assert!(SamlFederation::from_ctx(&ctx(Some("   "))).is_none());
    }
}

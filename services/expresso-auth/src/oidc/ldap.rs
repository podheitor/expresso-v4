//! LDAP federation detection from an `AuthContext`.
//!
//! Unlike SAML (identity brokering), an LDAP login is a *local* Keycloak user
//! backed by a User-Storage provider — it carries no `identity_provider` claim.
//! The discriminator is the `LDAP_ID` claim, exposed by a protocol-mapper
//! (seed §13) and present only on LDAP/AD-backed users. Mirrors `saml.rs`.

use expresso_auth_client::AuthContext;

/// Metadata about an LDAP-federated identity extracted from an `AuthContext`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LdapFederation {
    /// Keycloak `LDAP_ID` — the directory entry's stable id, key in
    /// `ldap_user_map`.
    pub ldap_id: String,
    /// Email mapped from the directory (may be empty if absent).
    pub email: String,
    /// Display name mapped from the directory (may be empty).
    pub display_name: String,
}

impl LdapFederation {
    /// Returns `Some` only when the token carries a non-empty `ldap_id` claim,
    /// i.e. the user is backed by an LDAP/AD user-storage provider.
    pub fn from_ctx(ctx: &AuthContext) -> Option<Self> {
        let ldap_id = ctx.ldap_id.clone()?;
        if ldap_id.trim().is_empty() {
            return None;
        }
        Some(Self {
            ldap_id,
            email: ctx.email.clone(),
            display_name: ctx.display_name.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn ctx(ldap_id: Option<&str>) -> AuthContext {
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
            identity_provider: None,
            ldap_id: ldap_id.map(String::from),
        }
    }

    #[test]
    fn some_when_ldap_id_present() {
        let f = LdapFederation::from_ctx(&ctx(Some("CN=alice,DC=corp"))).unwrap();
        assert_eq!(f.ldap_id, "CN=alice,DC=corp");
        assert_eq!(f.email, "u@corp.com");
        assert_eq!(f.display_name, "User");
    }

    #[test]
    fn none_when_ldap_id_absent() {
        assert!(LdapFederation::from_ctx(&ctx(None)).is_none());
    }

    #[test]
    fn none_when_ldap_id_blank() {
        assert!(LdapFederation::from_ctx(&ctx(Some("  "))).is_none());
    }
}

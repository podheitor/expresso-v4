//! SP-side SAML metadata for a Keycloak broker alias.
//!
//! In the Keycloak-brokered design KC is the SAML Service Provider, so the
//! metadata a customer's IdP admin needs points at KC's broker endpoints:
//! SP entityID `{kc_base}/realms/{realm}` and ACS endpoint
//! `{kc_base}/realms/{realm}/broker/{alias}/endpoint`.
//!
//! This is pure XML templating (no parsing, no signing, no XML library) — the
//! customer pastes the document into their IdP, or reads the two URLs off it.

/// Build the SP metadata XML for a tenant realm + broker alias. `kc_base` is the
/// externally reachable Keycloak base URL (no trailing slash). The values are
/// inserted into fixed attribute positions; `realm`/`alias` come from our own
/// config + DB (not arbitrary user input), so no escaping is performed.
pub fn build_sp_metadata(kc_base: &str, realm: &str, alias: &str) -> String {
    let base = kc_base.trim_end_matches('/');
    let entity_id = format!("{base}/realms/{realm}");
    let acs = format!("{base}/realms/{realm}/broker/{alias}/endpoint");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" entityID="{entity_id}">
  <md:SPSSODescriptor AuthnRequestsSigned="false" WantAssertionsSigned="true"
      protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <md:NameIDFormat>urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress</md:NameIDFormat>
    <md:AssertionConsumerService
        Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST"
        Location="{acs}" index="0" isDefault="true"/>
  </md:SPSSODescriptor>
</md:EntityDescriptor>
"#
    )
}

/// Derive the externally reachable Keycloak base URL (scheme + host[:port]) from
/// an issuer URL of the form `{kc_base}/realms/{realm}[/...]`. Returns `None`
/// when the issuer has no `/realms/` segment.
pub fn kc_base_from_issuer(issuer: &str) -> Option<&str> {
    let (base, _) = issuer.split_once("/realms/")?;
    if base.is_empty() {
        None
    } else {
        Some(base)
    }
}

/// Extract the realm segment from an issuer URL `{kc_base}/realms/{realm}[/...]`.
/// Returns `None` when there is no `/realms/` segment or it is empty / a
/// `{realm}` template placeholder (multi-realm mode resolves the realm from the
/// Host header instead).
pub fn realm_from_issuer(issuer: &str) -> Option<&str> {
    let (_, tail) = issuer.split_once("/realms/")?;
    let realm = tail.split('/').next()?;
    if realm.is_empty() || realm.contains('{') {
        None
    } else {
        Some(realm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_contains_entity_id_and_acs() {
        let xml = build_sp_metadata("https://kc.example.com", "tenant-uuid", "okta");
        assert!(xml.contains(r#"entityID="https://kc.example.com/realms/tenant-uuid""#));
        assert!(xml.contains(
            "Location=\"https://kc.example.com/realms/tenant-uuid/broker/okta/endpoint\""
        ));
    }

    #[test]
    fn metadata_strips_trailing_slash_on_base() {
        let xml = build_sp_metadata("https://kc.example.com/", "r", "azure");
        assert!(xml.contains(r#"entityID="https://kc.example.com/realms/r""#));
        assert!(!xml.contains("//realms"));
    }

    #[test]
    fn metadata_wants_assertions_signed() {
        let xml = build_sp_metadata("https://kc", "r", "a");
        assert!(xml.contains(r#"WantAssertionsSigned="true""#));
    }

    #[test]
    fn metadata_is_post_binding() {
        let xml = build_sp_metadata("https://kc", "r", "a");
        assert!(xml.contains("urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST"));
    }

    #[test]
    fn metadata_starts_with_xml_declaration() {
        let xml = build_sp_metadata("https://kc", "r", "a");
        assert!(xml.starts_with("<?xml version=\"1.0\""));
    }

    #[test]
    fn kc_base_from_issuer_strips_realm_path() {
        assert_eq!(
            kc_base_from_issuer("https://kc.example.com/realms/abc"),
            Some("https://kc.example.com")
        );
        assert_eq!(
            kc_base_from_issuer("http://kc:8080/realms/abc/protocol/openid-connect"),
            Some("http://kc:8080")
        );
    }

    #[test]
    fn kc_base_from_issuer_none_without_realms() {
        assert_eq!(kc_base_from_issuer("https://kc.example.com"), None);
        assert_eq!(kc_base_from_issuer(""), None);
    }

    #[test]
    fn realm_from_issuer_extracts_realm() {
        assert_eq!(
            realm_from_issuer("https://kc/realms/tenant-abc"),
            Some("tenant-abc")
        );
        assert_eq!(
            realm_from_issuer("http://kc:8080/realms/r/protocol/openid-connect"),
            Some("r")
        );
    }

    #[test]
    fn realm_from_issuer_none_for_template_or_missing() {
        assert_eq!(realm_from_issuer("https://kc/realms/{realm}"), None);
        assert_eq!(realm_from_issuer("https://kc/realms/"), None);
        assert_eq!(realm_from_issuer("https://kc"), None);
    }
}

-- Revert: drop the per-tenant SAML IdP configuration.

DROP TABLE IF EXISTS saml_idp_config;

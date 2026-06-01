-- Per-tenant SAML 2.0 IdP configuration.
--
-- Each tenant (realm) may register one or more external SAML identity providers,
-- which the admin service manages here and which are brokered by Keycloak (the
-- assertion parsing + XML-dsig verification happens inside KC, never in Rust —
-- mirroring the gov.br OIDC broker). `alias` matches the Keycloak
-- identity-provider instance alias used for `kc_idp_hint` at login.
--
-- Tenant scoping via explicit tenant_id column + WHERE (sibling admin
-- convention); admin handlers gate with require_tenant_match.

BEGIN;

CREATE TABLE IF NOT EXISTS saml_idp_config (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id        UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    alias            TEXT        NOT NULL,
    display_name     TEXT        NOT NULL,
    entity_id        TEXT        NOT NULL,
    sso_url          TEXT        NOT NULL,
    slo_url          TEXT,
    signing_cert     TEXT        NOT NULL,
    name_id_format   TEXT        NOT NULL DEFAULT 'urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress',
    attr_email       TEXT,
    attr_display_name TEXT,
    attr_given       TEXT,
    attr_family      TEXT,
    enabled          BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, alias)
);

CREATE INDEX IF NOT EXISTS idx_saml_idp_config_tenant ON saml_idp_config (tenant_id);

COMMIT;

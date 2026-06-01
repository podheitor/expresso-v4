-- Per-tenant LDAP / Active Directory federation configuration.
--
-- Each tenant (realm) may register one or more LDAP/AD directories, brokered by
-- Keycloak User Federation (the `components` API). Keycloak performs the LDAP
-- bind, search, sync, and password delegation; the Rust side stores this config
-- and reflects it into Keycloak. Mirrors saml_idp_config.
--
-- SECURITY: the LDAP bind-account password is NOT stored here. It is written
-- through to the Keycloak component only (KC is the source of truth for the
-- secret); this row holds everything except the credential. See the admin
-- handler and docs/LDAP_SYNC_PLAN.md.
--
-- Tenant scoping via explicit tenant_id column + WHERE (sibling admin
-- convention); admin handlers gate with require_tenant_match.

BEGIN;

CREATE TABLE IF NOT EXISTS ldap_config (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    alias               TEXT        NOT NULL,
    vendor              TEXT        NOT NULL DEFAULT 'other',
    connection_url      TEXT        NOT NULL,
    users_dn            TEXT        NOT NULL,
    bind_dn             TEXT        NOT NULL,
    username_attr       TEXT        NOT NULL DEFAULT 'uid',
    rdn_attr            TEXT        NOT NULL DEFAULT 'uid',
    uuid_attr           TEXT        NOT NULL DEFAULT 'entryUUID',
    user_object_classes TEXT        NOT NULL DEFAULT 'inetOrgPerson, organizationalPerson',
    search_scope        INTEGER     NOT NULL DEFAULT 2,
    sync_period_secs    INTEGER,
    enabled             BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, alias)
);

CREATE INDEX IF NOT EXISTS idx_ldap_config_tenant ON ldap_config (tenant_id);

COMMIT;

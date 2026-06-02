-- LDAP federation mapping: (tenant, LDAP_ID) → local user.
--
-- Populated by JIT provisioning on first LDAP-federated login. KC's LDAP_ID
-- (the directory entry's stable id) is the key, paired with tenant_id. Mirrors
-- saml_user_map but for User-Storage (LDAP) logins, which are not brokered and
-- so carry no identity_provider claim — the discriminator is the LDAP_ID claim.

BEGIN;

CREATE TABLE IF NOT EXISTS ldap_user_map (
    tenant_id     UUID        NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    ldap_id       TEXT        NOT NULL,
    user_id       UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, ldap_id)
);

CREATE INDEX IF NOT EXISTS idx_ldap_user_map_user ON ldap_user_map (user_id);

COMMIT;

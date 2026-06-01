-- SAML federation mapping: (tenant, idp, SAML subject) → local user.
--
-- Populated by the JIT auto-provisioning flow on first federated login
-- (sprint C): the auth service upserts the local `users` row and records the
-- binding here. Mirrors govbr_user_map. The SAML subject is the NameID emitted
-- by the IdP (opaque per IdP); paired with idp_alias + tenant_id it is unique.

BEGIN;

CREATE TABLE IF NOT EXISTS saml_user_map (
    tenant_id      UUID        NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    idp_alias      TEXT        NOT NULL,
    saml_subject   TEXT        NOT NULL,
    user_id        UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name_id_format TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at  TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, idp_alias, saml_subject)
);

CREATE INDEX IF NOT EXISTS idx_saml_user_map_tenant ON saml_user_map (tenant_id);
CREATE INDEX IF NOT EXISTS idx_saml_user_map_user   ON saml_user_map (user_id);

COMMIT;

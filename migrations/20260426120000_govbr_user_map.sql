-- gov.br federation mapping: cpf_hash → local user/tenant.
--
-- Populated by the admin provisioning flow after first gov.br federated login.
-- The auth service only emits audit events; it does not write this table.
-- Services that need to reconcile a gov.br citizen with a local tenant should
-- upsert here once provisioning rules are approved.
--
-- Privacy: cpf_hash is the hashed sub emitted by the gov.br IdP (never raw CPF).

BEGIN;

CREATE TABLE IF NOT EXISTS govbr_user_map (
    cpf_hash      TEXT PRIMARY KEY,
    tenant_id     UUID        NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    user_id       UUID        NOT NULL REFERENCES users(id)   ON DELETE CASCADE,
    assurance     TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_govbr_user_map_tenant ON govbr_user_map (tenant_id);
CREATE INDEX IF NOT EXISTS idx_govbr_user_map_user   ON govbr_user_map (user_id);

COMMIT;

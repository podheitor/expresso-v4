-- Mailbox delegation grants (phase 1: model + discovery).
--
-- An owner may grant another user in the same tenant delegated access to their
-- mailbox: READ (view mail) or SEND (send as the owner). This migration adds the
-- grant store and discovery only — the read/send paths still scope strictly to
-- the caller's own mailbox. Applying delegation to those paths (via an
-- on-behalf-of selector) is a later phase, mirroring how drive ACL landed
-- capability first, enforcement second.
--
-- Tenant isolation via RLS with the NULL-bypass bootstrap pattern used across
-- the schema. UNIQUE (tenant_id, owner_id, delegate_id) keeps one grant row per
-- pair; the access level is updated in place.

BEGIN;

CREATE TABLE IF NOT EXISTS mailbox_delegations (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    owner_id     UUID        NOT NULL REFERENCES users(id)   ON DELETE CASCADE,
    delegate_id  UUID        NOT NULL REFERENCES users(id)   ON DELETE CASCADE,
    access       TEXT        NOT NULL CHECK (access IN ('READ', 'SEND')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, owner_id, delegate_id),
    CHECK (owner_id <> delegate_id)
);

-- "My grants" (owner view) and "granted to me" (delegate view).
CREATE INDEX IF NOT EXISTS mailbox_delegations_owner_idx
    ON mailbox_delegations (tenant_id, owner_id);
CREATE INDEX IF NOT EXISTS mailbox_delegations_delegate_idx
    ON mailbox_delegations (tenant_id, delegate_id);

ALTER TABLE mailbox_delegations ENABLE ROW LEVEL SECURITY;
ALTER TABLE mailbox_delegations FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_mailbox_delegations ON mailbox_delegations;
CREATE POLICY rls_mailbox_delegations ON mailbox_delegations
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

COMMIT;

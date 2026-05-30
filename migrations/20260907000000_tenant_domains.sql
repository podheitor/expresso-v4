-- Migration: tenant domains
-- Registry of the mail domains a tenant owns. Gates which addresses a tenant
-- may claim (aliases, user mailboxes) and anchors DKIM key association. A
-- domain starts unverified with a random TXT token; verification (DNS lookup
-- of that token) flips is_verified — the DNS check itself is a follow-up, this
-- migration + API provide the registry and the verification state machine.

BEGIN;

CREATE TABLE IF NOT EXISTS tenant_domains (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    domain          TEXT NOT NULL,
    is_verified     BOOLEAN NOT NULL DEFAULT FALSE,
    -- Random token the tenant publishes as a DNS TXT record to prove ownership.
    verify_token    TEXT NOT NULL,
    verified_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- A domain is owned by at most one tenant, globally — two tenants cannot
    -- both claim @acme.com (unlike the per-tenant alias uniqueness).
    UNIQUE (domain)
);

CREATE INDEX IF NOT EXISTS idx_tenant_domains_tenant ON tenant_domains (tenant_id);

CREATE OR REPLACE FUNCTION tenant_domains_touch() RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS tr_tenant_domains_touch ON tenant_domains;
CREATE TRIGGER tr_tenant_domains_touch
    BEFORE UPDATE ON tenant_domains
    FOR EACH ROW EXECUTE FUNCTION tenant_domains_touch();

-- ─── RLS ─────────────────────────────────────────────────
-- Bootstrap-friendly: when app.tenant_id is NULL/empty, all rows visible
-- (migrations, superuser). Production MUST SET LOCAL app.tenant_id.
ALTER TABLE tenant_domains ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS tenant_domains_rls ON tenant_domains;
CREATE POLICY tenant_domains_rls ON tenant_domains
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR current_setting('app.tenant_id', true) = ''
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

COMMIT;

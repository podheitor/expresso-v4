-- Address book sharing / ACLs (mirror of calendar_acl).

BEGIN;

CREATE TABLE IF NOT EXISTS addressbook_acl (
    addressbook_id UUID        NOT NULL REFERENCES addressbooks(id) ON DELETE CASCADE,
    tenant_id      UUID        NOT NULL REFERENCES tenants(id)      ON DELETE CASCADE,
    grantee_id     UUID        NOT NULL REFERENCES users(id)        ON DELETE CASCADE,
    privilege      TEXT        NOT NULL CHECK (privilege IN ('READ','WRITE','ADMIN')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (addressbook_id, grantee_id)
);

CREATE INDEX IF NOT EXISTS idx_addressbook_acl_grantee ON addressbook_acl (grantee_id);
CREATE INDEX IF NOT EXISTS idx_addressbook_acl_tenant  ON addressbook_acl (tenant_id);

ALTER TABLE addressbook_acl ENABLE ROW LEVEL SECURITY;
ALTER TABLE addressbook_acl FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_addressbook_acl ON addressbook_acl;
CREATE POLICY rls_addressbook_acl ON addressbook_acl
    USING       (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK  (tenant_id = current_setting('app.tenant_id', true)::uuid);

COMMIT;

-- Calendar sharing / ACLs.
--
-- Per-user permissions on a calendar: a grantee may be READ (view events),
-- WRITE (create/update/delete their own entries in the cal), or ADMIN
-- (also manage ACL). Owner implicitly has ADMIN and does NOT appear here.
-- tenant_id is denormalised for RLS parity with calendars/events.

BEGIN;

CREATE TABLE IF NOT EXISTS calendar_acl (
    calendar_id  UUID        NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    tenant_id    UUID        NOT NULL REFERENCES tenants(id)   ON DELETE CASCADE,
    grantee_id   UUID        NOT NULL REFERENCES users(id)     ON DELETE CASCADE,
    privilege    TEXT        NOT NULL CHECK (privilege IN ('READ','WRITE','ADMIN')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (calendar_id, grantee_id)
);

CREATE INDEX IF NOT EXISTS idx_calendar_acl_grantee ON calendar_acl (grantee_id);
CREATE INDEX IF NOT EXISTS idx_calendar_acl_tenant  ON calendar_acl (tenant_id);

ALTER TABLE calendar_acl ENABLE ROW LEVEL SECURITY;
ALTER TABLE calendar_acl FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_calendar_acl ON calendar_acl;
CREATE POLICY rls_calendar_acl ON calendar_acl
    USING       (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK  (tenant_id = current_setting('app.tenant_id', true)::uuid);

COMMIT;

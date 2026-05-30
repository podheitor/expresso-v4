-- Drive file/folder sharing — per-user ACLs (mirror of calendar_acl).
--
-- A grantee may be READ (view + download), WRITE (also rename/move/update/
-- delete), or ADMIN (also manage the ACL). The owner (drive_files.owner_user_id)
-- implicitly has ADMIN and does NOT appear here. Works for both files and
-- folders — the node is a drive_files row either way; a grant on a folder is
-- intended to cover its descendants (enforced in application code, not SQL).
-- tenant_id is denormalised for RLS parity with drive_files.

BEGIN;

CREATE TABLE IF NOT EXISTS drive_file_acl (
    file_id      UUID        NOT NULL REFERENCES drive_files(id) ON DELETE CASCADE,
    tenant_id    UUID        NOT NULL REFERENCES tenants(id)     ON DELETE CASCADE,
    grantee_id   UUID        NOT NULL REFERENCES users(id)       ON DELETE CASCADE,
    privilege    TEXT        NOT NULL CHECK (privilege IN ('READ','WRITE','ADMIN')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (file_id, grantee_id)
);

CREATE INDEX IF NOT EXISTS idx_drive_file_acl_grantee ON drive_file_acl (grantee_id);
CREATE INDEX IF NOT EXISTS idx_drive_file_acl_tenant  ON drive_file_acl (tenant_id);

ALTER TABLE drive_file_acl ENABLE ROW LEVEL SECURITY;
ALTER TABLE drive_file_acl FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_drive_file_acl ON drive_file_acl;
CREATE POLICY rls_drive_file_acl ON drive_file_acl
    USING       (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK  (tenant_id = current_setting('app.tenant_id', true)::uuid);

COMMIT;

-- Notes sharing / ACLs.
--
-- Per-user permissions on a note: a grantee may be READ (view), WRITE (edit),
-- or ADMIN (also manage the ACL). The owner (notes.user_id) implicitly has full
-- access and does NOT appear here. tenant_id is denormalised for RLS parity with
-- notes, mirroring calendar_acl.

CREATE TABLE IF NOT EXISTS notes_acl (
    note_id     UUID        NOT NULL REFERENCES notes(id)   ON DELETE CASCADE,
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    grantee_id  UUID        NOT NULL REFERENCES users(id)   ON DELETE CASCADE,
    privilege   TEXT        NOT NULL CHECK (privilege IN ('READ','WRITE','ADMIN')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (note_id, grantee_id)
);

CREATE INDEX IF NOT EXISTS idx_notes_acl_grantee ON notes_acl (grantee_id);
CREATE INDEX IF NOT EXISTS idx_notes_acl_tenant  ON notes_acl (tenant_id);

ALTER TABLE notes_acl ENABLE ROW LEVEL SECURITY;
ALTER TABLE notes_acl FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_notes_acl ON notes_acl;
CREATE POLICY rls_notes_acl ON notes_acl
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

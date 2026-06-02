-- Notes activity log (audit trail) — mirrors contact_activity / drive_file_activity.
--
-- Append-only record of note mutations (create/update/delete) and any explicitly
-- recorded events, so the notes service offers the same per-object audit trail
-- Contacts/Drive already have. Tenant isolation via RLS, matching the other
-- notes tables (notes/notes_tags/notes_acl).

CREATE TABLE IF NOT EXISTS notes_activity (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    note_id     UUID        NOT NULL,
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id     UUID        NOT NULL,
    action      TEXT        NOT NULL,   -- create | update | delete | restore
    detail      JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Newest-first listing per note, and a per-user audit view.
CREATE INDEX IF NOT EXISTS notes_activity_note_idx
    ON notes_activity (tenant_id, note_id, created_at DESC);
CREATE INDEX IF NOT EXISTS notes_activity_user_idx
    ON notes_activity (tenant_id, user_id, created_at DESC);

ALTER TABLE notes_activity ENABLE ROW LEVEL SECURITY;
ALTER TABLE notes_activity FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_notes_activity ON notes_activity;
CREATE POLICY rls_notes_activity ON notes_activity
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

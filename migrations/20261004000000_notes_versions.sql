-- Notes version history.
--
-- Each update snapshots the note's prior title/body/color into notes_versions
-- before the live row changes, so a user can review and restore earlier text
-- (OneNote/Keep parity). version_no is monotonically increasing per note. The
-- snapshot captures content only — pinned/archived/notebook are view state, not
-- content, and are intentionally excluded. Tenant isolation via RLS, matching
-- notes / notes_acl / notes_tags.

CREATE TABLE IF NOT EXISTS notes_versions (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    note_id     UUID        NOT NULL REFERENCES notes(id)   ON DELETE CASCADE,
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    version_no  INTEGER     NOT NULL CHECK (version_no >= 1),
    title       TEXT        NOT NULL,
    body        TEXT        NOT NULL,
    color       TEXT,
    edited_by   UUID        NOT NULL REFERENCES users(id)   ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (note_id, version_no)
);

-- List a note's versions, newest first.
CREATE INDEX IF NOT EXISTS notes_versions_note_idx
    ON notes_versions (tenant_id, note_id, version_no DESC);

ALTER TABLE notes_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE notes_versions FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_notes_versions ON notes_versions;
CREATE POLICY rls_notes_versions ON notes_versions
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

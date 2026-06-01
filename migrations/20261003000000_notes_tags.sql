-- Notes tags (labels).
--
-- A note may carry free-form tags (labels), many-to-many, orthogonal to
-- notebooks: a note lives in at most one notebook but can hold several tags.
-- Tags are per-note; the same tag string may recur across a user's notes and is
-- how the notes list filters by `?tag=`. Tenant isolation via RLS, matching
-- `notes` / `notes_acl`.

CREATE TABLE IF NOT EXISTS notes_tags (
    note_id     UUID        NOT NULL REFERENCES notes(id)   ON DELETE CASCADE,
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    tag         TEXT        NOT NULL CHECK (char_length(tag) BETWEEN 1 AND 64),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (note_id, tag)
);

-- List a note's tags; and find notes by tag within a tenant.
CREATE INDEX IF NOT EXISTS notes_tags_note_idx ON notes_tags (tenant_id, note_id);
CREATE INDEX IF NOT EXISTS notes_tags_tag_idx  ON notes_tags (tenant_id, tag);

ALTER TABLE notes_tags ENABLE ROW LEVEL SECURITY;
ALTER TABLE notes_tags FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_notes_tags ON notes_tags;
CREATE POLICY rls_notes_tags ON notes_tags
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

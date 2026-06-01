-- Notes notebooks (folders).
--
-- A notebook groups a user's notes (OneNote/Keep parity). Owned by a user
-- within a tenant, mirroring `notes` ownership. A note references at most one
-- notebook via `notes.notebook_id`; deleting a notebook detaches its notes
-- (SET NULL) rather than cascading them away — they fall back to "no notebook".
-- Tenant isolation via RLS on `app.tenant_id`, matching `notes`.

CREATE TABLE IF NOT EXISTS notes_notebooks (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    user_id     UUID        NOT NULL REFERENCES users (id)   ON DELETE CASCADE,
    name        TEXT        NOT NULL,
    color       TEXT,                              -- optional hex swatch, e.g. #4dd0e1
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- List a user's notebooks, newest first.
CREATE INDEX IF NOT EXISTS notes_notebooks_owner_idx
    ON notes_notebooks (tenant_id, user_id, created_at DESC);

CREATE TRIGGER notes_notebooks_touch_trg
    BEFORE UPDATE ON notes_notebooks
    FOR EACH ROW EXECUTE FUNCTION notes_touch();

ALTER TABLE notes_notebooks ENABLE ROW LEVEL SECURITY;
ALTER TABLE notes_notebooks FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_notes_notebooks ON notes_notebooks;
CREATE POLICY rls_notes_notebooks ON notes_notebooks
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

-- Attach notes to a notebook. NULL = loose note (default). On notebook delete
-- the note detaches (SET NULL), preserving the note itself.
ALTER TABLE notes
    ADD COLUMN IF NOT EXISTS notebook_id UUID
        REFERENCES notes_notebooks (id) ON DELETE SET NULL;

-- Filter a notebook's notes for a user (partial: only attached notes).
CREATE INDEX IF NOT EXISTS notes_notebook_idx
    ON notes (tenant_id, user_id, notebook_id)
    WHERE notebook_id IS NOT NULL;

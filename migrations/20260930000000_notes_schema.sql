-- expresso-notes: per-user notes/memos.
--
-- A note is owned by a user within a tenant — the simplest personal-notes model
-- (no sharing in this first slice). Tenant isolation via RLS on `app.tenant_id`,
-- mirroring calendars/contacts; ownership is enforced by an explicit user_id
-- predicate in the repo on top of RLS.

CREATE TABLE IF NOT EXISTS notes (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    user_id     UUID        NOT NULL REFERENCES users (id)   ON DELETE CASCADE,
    title       TEXT        NOT NULL DEFAULT '',
    body        TEXT        NOT NULL DEFAULT '',
    color       TEXT,                              -- optional hex swatch, e.g. #ffd54f
    pinned      BOOLEAN     NOT NULL DEFAULT FALSE,
    archived    BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- List ordering / filtering: a user's live notes, pinned first, newest first.
CREATE INDEX IF NOT EXISTS notes_owner_idx
    ON notes (tenant_id, user_id, archived, pinned DESC, updated_at DESC);

CREATE OR REPLACE FUNCTION notes_touch() RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER notes_touch_trg
    BEFORE UPDATE ON notes
    FOR EACH ROW EXECUTE FUNCTION notes_touch();

ALTER TABLE notes ENABLE ROW LEVEL SECURITY;
ALTER TABLE notes FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_notes ON notes;
CREATE POLICY rls_notes ON notes
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

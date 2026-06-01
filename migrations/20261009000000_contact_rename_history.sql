-- Contact rename history + undo.
--
-- A REST update that changes a contact's FN (display name / full_name) records
-- (old, new) so a user can review past renames and undo one — mirroring
-- drive/calendar/mail rename-history. Logged on the REST update path where the
-- acting user is known; CardDAV writes (no per-field user intent) are not
-- tracked here. Tenant scoping via explicit columns + WHERE.

CREATE TABLE IF NOT EXISTS contact_rename_history (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID        NOT NULL,
    contact_id   UUID        NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
    old_name     TEXT        NOT NULL,
    new_name     TEXT        NOT NULL,
    renamed_by   UUID        NOT NULL,
    renamed_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS contact_rename_history_contact_idx
    ON contact_rename_history (tenant_id, contact_id, renamed_at DESC);

-- Contact version history.
--
-- A REST update snapshots the contact's prior vCard into contact_versions so a
-- user can review past revisions and restore one — mirroring notes_versions and
-- drive_file_versions. Because a contact is stored as an opaque vCard blob, the
-- snapshot is the whole `vcard_raw` (the source of truth) rather than per-field.
-- `version_no` is assigned per contact as max+1 inside the snapshot tx, so
-- concurrent edits get distinct numbers. Logged on the REST update path where
-- the acting user is known; CardDAV writes (no per-field user intent) are not
-- versioned here. Tenant scoping via explicit columns + WHERE (sibling
-- child-table convention, no RLS).

CREATE TABLE IF NOT EXISTS contact_versions (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID        NOT NULL,
    contact_id   UUID        NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
    version_no   INTEGER     NOT NULL,
    vcard_raw    TEXT        NOT NULL,
    full_name    TEXT,
    edited_by    UUID        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, contact_id, version_no)
);

CREATE INDEX IF NOT EXISTS contact_versions_contact_idx
    ON contact_versions (tenant_id, contact_id, version_no DESC);

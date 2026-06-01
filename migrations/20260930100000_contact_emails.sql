-- Indexed secondary email addresses for contacts (RFC 6350 §6.4.2).
--
-- A contact's full vCard lives in contacts.vcard_raw; the primary EMAIL is
-- denormalized onto contacts.email_primary. This table indexes ALL of a
-- contact's EMAIL entries (incl. the primary) with their TYPE label, so a
-- search can match any address and clients can list them without re-parsing.
-- Rows are (re)synced by ContactRepo on create/update: delete-all-then-insert
-- in the same tx, mirroring the calendar child-table convention. Tenant scoping
-- via explicit tenant_id column + WHERE filter.

CREATE TABLE IF NOT EXISTS contact_emails (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL,
    contact_id  UUID        NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
    address     TEXT        NOT NULL,
    label       TEXT,                              -- TYPE (work/home/…), or NULL
    position    INTEGER     NOT NULL DEFAULT 0,    -- order within the vCard
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS contact_emails_contact_idx
    ON contact_emails (tenant_id, contact_id, position);
-- Search any address across the tenant.
CREATE INDEX IF NOT EXISTS contact_emails_addr_idx
    ON contact_emails (tenant_id, address);

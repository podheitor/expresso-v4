-- Indexed postal addresses for contacts (RFC 6350 §6.3.1 ADR).
--
-- The contact's full vCard lives in contacts.vcard_raw; this table indexes each
-- ADR's structured components + TYPE label so clients can list addresses (and a
-- search can match locality/postal) without re-parsing the blob. Rows are
-- (re)synced by ContactRepo on create/update: delete-all-then-insert in the same
-- tx, mirroring contact_emails. Tenant scoping via explicit columns + WHERE.

CREATE TABLE IF NOT EXISTS contact_addresses (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID        NOT NULL,
    contact_id   UUID        NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
    label        TEXT,                              -- TYPE (work/home/…)
    po_box       TEXT,
    ext          TEXT,
    street       TEXT,
    locality     TEXT,
    region       TEXT,
    postal_code  TEXT,
    country      TEXT,
    position     INTEGER     NOT NULL DEFAULT 0,    -- order within the vCard
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS contact_addresses_contact_idx
    ON contact_addresses (tenant_id, contact_id, position);

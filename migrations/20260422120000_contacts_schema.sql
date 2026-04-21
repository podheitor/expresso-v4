-- Migration: contacts schema
-- Addressbooks (collections) + contacts (vCard payloads) with RLS.
-- Mirrors the calendar schema: tenant_id FK, RLS via app.tenant_id,
-- updated_at auto-bump, ctag bump on child mutation for CardDAV sync.

BEGIN;

-- ─── ADDRESSBOOKS ────────────────────────────────────────
CREATE TABLE IF NOT EXISTS addressbooks (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    owner_user_id   UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT,
    ctag            BIGINT NOT NULL DEFAULT 1,      -- CardDAV collection tag
    is_default      BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_addressbooks_tenant ON addressbooks (tenant_id);
CREATE INDEX IF NOT EXISTS idx_addressbooks_owner  ON addressbooks (owner_user_id);
-- At most one default addressbook per owner
CREATE UNIQUE INDEX IF NOT EXISTS uq_addressbooks_owner_default
    ON addressbooks (owner_user_id) WHERE is_default;

-- ─── CONTACTS ────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS contacts (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    addressbook_id  UUID NOT NULL REFERENCES addressbooks (id) ON DELETE CASCADE,
    tenant_id       UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    uid             TEXT NOT NULL,                  -- vCard UID (RFC 6350)
    etag            TEXT NOT NULL,                  -- sha256(vcard_raw)
    vcard_raw       TEXT NOT NULL,                  -- full vCard 3.0/4.0 payload
    -- Denormalised fields (fast search + listing).
    full_name       TEXT,                           -- FN
    family_name     TEXT,                           -- N[0]
    given_name      TEXT,                           -- N[1]
    organization    TEXT,                           -- ORG
    email_primary   TEXT,                           -- first EMAIL
    phone_primary   TEXT,                           -- first TEL
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_contacts_book_uid
    ON contacts (addressbook_id, uid);
CREATE INDEX IF NOT EXISTS idx_contacts_book       ON contacts (addressbook_id);
CREATE INDEX IF NOT EXISTS idx_contacts_tenant     ON contacts (tenant_id);
CREATE INDEX IF NOT EXISTS idx_contacts_email      ON contacts (email_primary);
CREATE INDEX IF NOT EXISTS idx_contacts_fullname   ON contacts (full_name);

-- ─── ctag auto-bump + updated_at touch ───────────────────
CREATE OR REPLACE FUNCTION addressbooks_touch() RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS tr_addressbooks_touch ON addressbooks;
CREATE TRIGGER tr_addressbooks_touch
    BEFORE UPDATE ON addressbooks
    FOR EACH ROW EXECUTE FUNCTION addressbooks_touch();

-- Bumps parent addressbook ctag whenever a child contact changes.
CREATE OR REPLACE FUNCTION contacts_touch() RETURNS TRIGGER AS $$
DECLARE
    target_book UUID;
BEGIN
    target_book := COALESCE(NEW.addressbook_id, OLD.addressbook_id);
    UPDATE addressbooks
       SET ctag       = ctag + 1,
           updated_at = now()
     WHERE id = target_book;
    IF TG_OP = 'UPDATE' THEN
        NEW.updated_at := now();
    END IF;
    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS tr_contacts_touch ON contacts;
CREATE TRIGGER tr_contacts_touch
    BEFORE INSERT OR UPDATE OR DELETE ON contacts
    FOR EACH ROW EXECUTE FUNCTION contacts_touch();

-- ─── RLS ─────────────────────────────────────────────────
-- Bootstrap-friendly: when app.tenant_id is NULL, all rows visible (migrations,
-- superuser). Production apps MUST SET LOCAL app.tenant_id = '<uuid>'.
ALTER TABLE addressbooks ENABLE ROW LEVEL SECURITY;
ALTER TABLE contacts     ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS addressbooks_rls ON addressbooks;
CREATE POLICY addressbooks_rls ON addressbooks
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR current_setting('app.tenant_id', true) = ''
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

DROP POLICY IF EXISTS contacts_rls ON contacts;
CREATE POLICY contacts_rls ON contacts
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR current_setting('app.tenant_id', true) = ''
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

COMMIT;

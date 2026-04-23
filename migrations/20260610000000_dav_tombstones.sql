-- ═══════════════════════════════════════════════════════════
-- DAV sync-collection (RFC 6578) delta support.
-- Adds per-row ctag versioning + tombstones for CalDAV + CardDAV.
-- ═══════════════════════════════════════════════════════════

-- ─── CALENDAR EVENTS: last_ctag + tombstones ───────────────
ALTER TABLE calendar_events
    ADD COLUMN IF NOT EXISTS last_ctag BIGINT NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_events_calendar_last_ctag
    ON calendar_events (calendar_id, last_ctag);

CREATE TABLE IF NOT EXISTS calendar_event_tombstones (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     UUID NOT NULL,
    calendar_id   UUID NOT NULL,
    uid           TEXT NOT NULL,
    deleted_ctag  BIGINT NOT NULL,
    deleted_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_event_tombstones_sync
    ON calendar_event_tombstones (calendar_id, deleted_ctag);
CREATE INDEX IF NOT EXISTS idx_event_tombstones_tenant
    ON calendar_event_tombstones (tenant_id);

-- Replace trigger fn → stamp last_ctag on INSERT/UPDATE, write tombstone on DELETE.
CREATE OR REPLACE FUNCTION calendar_events_touch()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    new_ctag BIGINT;
BEGIN
    IF TG_OP = 'DELETE' THEN
        UPDATE calendars
           SET ctag = ctag + 1,
               updated_at = now()
         WHERE id = OLD.calendar_id
         RETURNING ctag INTO new_ctag;

        INSERT INTO calendar_event_tombstones (tenant_id, calendar_id, uid, deleted_ctag)
        VALUES (OLD.tenant_id, OLD.calendar_id, OLD.uid, new_ctag);

        RETURN OLD;
    ELSE
        IF TG_OP = 'UPDATE' THEN
            NEW.updated_at := now();
        END IF;

        UPDATE calendars
           SET ctag = ctag + 1,
               updated_at = now()
         WHERE id = NEW.calendar_id
         RETURNING ctag INTO new_ctag;

        NEW.last_ctag := new_ctag;
        RETURN NEW;
    END IF;
END;
$$;

-- ─── CONTACTS: last_ctag + tombstones ──────────────────────
ALTER TABLE contacts
    ADD COLUMN IF NOT EXISTS last_ctag BIGINT NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_contacts_book_last_ctag
    ON contacts (addressbook_id, last_ctag);

CREATE TABLE IF NOT EXISTS contact_tombstones (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL,
    addressbook_id  UUID NOT NULL,
    uid             TEXT NOT NULL,
    deleted_ctag    BIGINT NOT NULL,
    deleted_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_contact_tombstones_sync
    ON contact_tombstones (addressbook_id, deleted_ctag);
CREATE INDEX IF NOT EXISTS idx_contact_tombstones_tenant
    ON contact_tombstones (tenant_id);

CREATE OR REPLACE FUNCTION contacts_touch() RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    new_ctag BIGINT;
BEGIN
    IF TG_OP = 'DELETE' THEN
        UPDATE addressbooks
           SET ctag = ctag + 1,
               updated_at = now()
         WHERE id = OLD.addressbook_id
         RETURNING ctag INTO new_ctag;

        INSERT INTO contact_tombstones (tenant_id, addressbook_id, uid, deleted_ctag)
        VALUES (OLD.tenant_id, OLD.addressbook_id, OLD.uid, new_ctag);

        RETURN OLD;
    ELSE
        IF TG_OP = 'UPDATE' THEN
            NEW.updated_at := now();
        END IF;

        UPDATE addressbooks
           SET ctag = ctag + 1,
               updated_at = now()
         WHERE id = NEW.addressbook_id
         RETURNING ctag INTO new_ctag;

        NEW.last_ctag := new_ctag;
        RETURN NEW;
    END IF;
END;
$$;

-- ─── Backfill last_ctag on existing rows ───────────────────
-- Set to parent's current ctag so existing rows appear as "changed at current ctag".
UPDATE calendar_events ce
   SET last_ctag = c.ctag
  FROM calendars c
 WHERE ce.calendar_id = c.id
   AND ce.last_ctag = 0;

UPDATE contacts ct
   SET last_ctag = ab.ctag
  FROM addressbooks ab
 WHERE ct.addressbook_id = ab.id
   AND ct.last_ctag = 0;

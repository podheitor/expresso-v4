-- Migration: calendar schema
-- Calendars (collections) + calendar_events (VEVENT payloads) with RLS
-- Conventions mirrored from mail/core: tenant_id FK, RLS via app.tenant_id,
-- updated_at auto-bump, ctag bump on child mutation for CalDAV sync.

BEGIN;

-- ─── CALENDARS ───────────────────────────────────────────
CREATE TABLE IF NOT EXISTS calendars (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    owner_user_id   UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT,
    color           TEXT,                           -- hex, e.g. #3498db
    timezone        TEXT NOT NULL DEFAULT 'America/Sao_Paulo',
    ctag            BIGINT NOT NULL DEFAULT 1,      -- CalDAV collection tag
    is_default      BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_calendars_tenant ON calendars (tenant_id);
CREATE INDEX IF NOT EXISTS idx_calendars_owner  ON calendars (owner_user_id);
-- At most one default calendar per owner
CREATE UNIQUE INDEX IF NOT EXISTS uq_calendars_owner_default
    ON calendars (owner_user_id) WHERE is_default;

-- ─── CALENDAR EVENTS ─────────────────────────────────────
CREATE TABLE IF NOT EXISTS calendar_events (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    calendar_id     UUID NOT NULL REFERENCES calendars (id) ON DELETE CASCADE,
    tenant_id       UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    uid             TEXT NOT NULL,                  -- iCalendar UID (RFC 5545)
    etag            TEXT NOT NULL,                  -- sha256(ical_raw)
    ical_raw        TEXT NOT NULL,                  -- full VCALENDAR wrapping VEVENT(s)
    summary         TEXT,
    description     TEXT,
    location        TEXT,
    dtstart         TIMESTAMPTZ,
    dtend           TIMESTAMPTZ,
    rrule           TEXT,                           -- raw RRULE string
    status          TEXT CHECK (status IN ('TENTATIVE','CONFIRMED','CANCELLED') OR status IS NULL),
    sequence        INTEGER NOT NULL DEFAULT 0,
    organizer_email TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_events_calendar_uid
    ON calendar_events (calendar_id, uid);
CREATE INDEX IF NOT EXISTS idx_events_tenant ON calendar_events (tenant_id);
CREATE INDEX IF NOT EXISTS idx_events_calendar_dtstart
    ON calendar_events (calendar_id, dtstart);
CREATE INDEX IF NOT EXISTS idx_events_dtrange
    ON calendar_events (calendar_id, dtstart, dtend);

-- ─── TRIGGERS ────────────────────────────────────────────
-- Bump parent calendar ctag + updated_at on any event mutation; touch event.updated_at on UPDATE.
CREATE OR REPLACE FUNCTION calendar_events_touch()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        NEW.updated_at := now();
    END IF;

    IF TG_OP = 'DELETE' THEN
        UPDATE calendars
           SET ctag = ctag + 1,
               updated_at = now()
         WHERE id = OLD.calendar_id;
        RETURN OLD;
    ELSE
        UPDATE calendars
           SET ctag = ctag + 1,
               updated_at = now()
         WHERE id = NEW.calendar_id;
        RETURN NEW;
    END IF;
END;
$$;

DROP TRIGGER IF EXISTS trg_calendar_events_touch ON calendar_events;
CREATE TRIGGER trg_calendar_events_touch
    BEFORE INSERT OR UPDATE OR DELETE ON calendar_events
    FOR EACH ROW EXECUTE FUNCTION calendar_events_touch();

-- Touch calendar.updated_at on update
CREATE OR REPLACE FUNCTION calendars_touch()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_calendars_touch ON calendars;
CREATE TRIGGER trg_calendars_touch
    BEFORE UPDATE ON calendars
    FOR EACH ROW EXECUTE FUNCTION calendars_touch();

-- ─── ROW LEVEL SECURITY ──────────────────────────────────
ALTER TABLE calendars       ENABLE ROW LEVEL SECURITY;
ALTER TABLE calendar_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE calendars       FORCE  ROW LEVEL SECURITY;
ALTER TABLE calendar_events FORCE  ROW LEVEL SECURITY;

-- Mirror bootstrap-friendly policies (NULL app.tenant_id allowed for service tooling)
DROP POLICY IF EXISTS rls_calendars ON calendars;
CREATE POLICY rls_calendars ON calendars
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

DROP POLICY IF EXISTS rls_calendar_events ON calendar_events;
CREATE POLICY rls_calendar_events ON calendar_events
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

COMMIT;

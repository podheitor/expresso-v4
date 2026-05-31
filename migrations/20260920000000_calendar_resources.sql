-- Calendar resources (bookable rooms/equipment) + per-event booking index.
--
-- A "resource" is a bookable entity (meeting room, projector) identified by an
-- email, matching the ATTENDEE;CUTYPE=ROOM|RESOURCE convention (RFC 5545
-- §3.2.3). `calendar_resources` is the tenant's registry; `calendar_event_
-- resources` indexes which resource an event books, (re)synced from the event's
-- CUTYPE=ROOM/RESOURCE attendees on save (delete-then-insert in-tx, mirroring
-- calendar_event_attachments). Double-booking = two events overlapping in time
-- that both reference the same resource_email.
--
-- Scope note: bookings are matched by email and compared on stored dtstart/dtend
-- only (no RRULE expansion), consistent with the events-conflicts endpoint.
-- Tenant scoping follows sibling calendar tables: explicit tenant_id + WHERE.

CREATE TABLE IF NOT EXISTS calendar_resources (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL,
    email       TEXT        NOT NULL,                 -- mailto identity in ATTENDEE
    name        TEXT        NOT NULL,
    kind        TEXT        NOT NULL DEFAULT 'room'   -- room | resource
                CHECK (kind IN ('room', 'resource')),
    capacity    INTEGER,                              -- seats (rooms); NULL = n/a
    is_active   BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, email)
);

CREATE INDEX IF NOT EXISTS calendar_resources_tenant_idx
    ON calendar_resources (tenant_id, is_active);

-- Per-event index of booked resources. resource_email mirrors the event's
-- CUTYPE=ROOM/RESOURCE attendees; it is NOT a FK to calendar_resources so a
-- booking of an unregistered room is still recorded (and surfaced as such).
CREATE TABLE IF NOT EXISTS calendar_event_resources (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      UUID        NOT NULL,
    calendar_id    UUID        NOT NULL,
    event_id       UUID        NOT NULL REFERENCES calendar_events(id) ON DELETE CASCADE,
    resource_email TEXT        NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS calendar_event_resources_email_idx
    ON calendar_event_resources (tenant_id, resource_email);
CREATE INDEX IF NOT EXISTS calendar_event_resources_event_idx
    ON calendar_event_resources (tenant_id, event_id);

-- updated_at trigger for the registry (reuses the meetings_touch pattern).
CREATE OR REPLACE FUNCTION calendar_resources_touch() RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER calendar_resources_touch_trg
    BEFORE UPDATE ON calendar_resources
    FOR EACH ROW EXECUTE FUNCTION calendar_resources_touch();

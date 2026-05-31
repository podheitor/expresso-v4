-- Calendar event attachments — indexed ATTACH properties (RFC 5545 §3.8.1.1).
--
-- An event's full VCALENDAR (incl. raw ATTACH bytes for inline binaries) lives
-- in calendar_events.ical_raw; this table only indexes ATTACH *metadata* so a
-- client can list an event's attachments without re-parsing the blob. Rows are
-- (re)synced by EventRepo on create/update: delete-all-then-insert within the
-- same tx, so the index always mirrors the current ical_raw.
--
-- `uri` holds the external reference for URI-form ATTACH; it is NULL for inline
-- binary (is_inline = TRUE) whose bytes stay in ical_raw — never copied here.
-- Tenant scoping follows the sibling calendar child tables (alarms/history):
-- explicit tenant_id column + WHERE filter, no RLS policy.

CREATE TABLE IF NOT EXISTS calendar_event_attachments (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL,
    calendar_id UUID        NOT NULL,
    event_id    UUID        NOT NULL REFERENCES calendar_events(id) ON DELETE CASCADE,
    uri         TEXT,                              -- external ref; NULL for inline
    fmttype     TEXT,                              -- MIME type (FMTTYPE param)
    is_inline   BOOLEAN     NOT NULL DEFAULT FALSE,
    position    INTEGER     NOT NULL DEFAULT 0,    -- order within the VEVENT
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS calendar_event_attachments_event_idx
    ON calendar_event_attachments (tenant_id, event_id, position);

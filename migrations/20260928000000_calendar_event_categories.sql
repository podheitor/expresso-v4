-- Calendar event categories — indexed CATEGORIES properties (RFC 5545 §3.8.1.2).
--
-- An event's full VCALENDAR lives in calendar_events.ical_raw; this table
-- indexes its CATEGORIES (tags like "work", "urgent") so a client can list or
-- filter by category without re-parsing the blob. CATEGORIES is a comma-
-- separated value per property line; each value becomes one row. Rows are
-- (re)synced by EventRepo on create/update: delete-all-then-insert in the same
-- tx, mirroring calendar_event_attachments. Tenant scoping via explicit
-- tenant_id column + WHERE filter (sibling child-table convention, no RLS).

CREATE TABLE IF NOT EXISTS calendar_event_categories (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL,
    calendar_id UUID        NOT NULL,
    event_id    UUID        NOT NULL REFERENCES calendar_events(id) ON DELETE CASCADE,
    category    TEXT        NOT NULL,
    position    INTEGER     NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS calendar_event_categories_event_idx
    ON calendar_event_categories (tenant_id, event_id, position);

-- Find events by category within a tenant (filtering UI).
CREATE INDEX IF NOT EXISTS calendar_event_categories_cat_idx
    ON calendar_event_categories (tenant_id, category);

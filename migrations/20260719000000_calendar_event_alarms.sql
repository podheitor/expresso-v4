-- Sprint #401: calendar event alarms (VALARM REST API)
CREATE TABLE IF NOT EXISTS calendar_event_alarms (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id    UUID NOT NULL,
    calendar_id UUID NOT NULL,
    tenant_id   UUID NOT NULL,
    uid         TEXT NOT NULL,
    action      TEXT NOT NULL DEFAULT 'DISPLAY',   -- DISPLAY | AUDIO | EMAIL
    trigger_rel TEXT,                               -- e.g. -PT15M (relative to DTSTART)
    trigger_abs TIMESTAMPTZ,                        -- absolute trigger time (alternative)
    description TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (event_id, uid)
);

CREATE INDEX IF NOT EXISTS calendar_event_alarms_event_idx
    ON calendar_event_alarms (tenant_id, event_id);

-- Calendar tasks (RFC 5545 VTODO).
--
-- A task is a to-do owned by a calendar, sharing that calendar's tenant +
-- ACL model (the same READ/WRITE/ADMIN grants that gate events apply). This is
-- the storage behind the VTODO component the CalDAV layer already advertises in
-- supported-calendar-component-set; the first slice exposes REST CRUD, with
-- CalDAV VTODO serialization to follow.
--
-- status mirrors VTODO's STATUS property (RFC 5545 §3.8.1.11). percent_complete
-- is 0..100 (PERCENT-COMPLETE). due/dtstart are optional; a task need not be
-- time-bound. completed_at records the COMPLETED timestamp when status flips to
-- COMPLETED. Tenant scoping is enforced by RLS (app.tenant_id), matching
-- calendars/calendar_events.

CREATE TABLE IF NOT EXISTS calendar_tasks (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id        UUID        NOT NULL REFERENCES tenants (id)   ON DELETE CASCADE,
    calendar_id      UUID        NOT NULL REFERENCES calendars (id) ON DELETE CASCADE,
    uid              TEXT        NOT NULL,                  -- iCalendar UID (VTODO)
    summary          TEXT        NOT NULL,
    description      TEXT,
    status           TEXT        NOT NULL DEFAULT 'NEEDS-ACTION'
                     CHECK (status IN ('NEEDS-ACTION', 'IN-PROCESS',
                                       'COMPLETED', 'CANCELLED')),
    priority         SMALLINT    NOT NULL DEFAULT 0          -- 0=undefined, 1=high..9=low
                     CHECK (priority BETWEEN 0 AND 9),
    percent_complete SMALLINT    NOT NULL DEFAULT 0
                     CHECK (percent_complete BETWEEN 0 AND 100),
    dtstart          TIMESTAMPTZ,
    due              TIMESTAMPTZ,
    completed_at     TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (calendar_id, uid)
);

CREATE INDEX IF NOT EXISTS calendar_tasks_cal_idx
    ON calendar_tasks (tenant_id, calendar_id, status);
CREATE INDEX IF NOT EXISTS calendar_tasks_due_idx
    ON calendar_tasks (tenant_id, due);

-- updated_at trigger (reuses the sibling-table touch pattern).
CREATE OR REPLACE FUNCTION calendar_tasks_touch() RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER calendar_tasks_touch_trg
    BEFORE UPDATE ON calendar_tasks
    FOR EACH ROW EXECUTE FUNCTION calendar_tasks_touch();

-- Row level security: tenant isolation via app.tenant_id (bootstrap-friendly,
-- mirroring calendars/calendar_events).
ALTER TABLE calendar_tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE calendar_tasks FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_calendar_tasks ON calendar_tasks;
CREATE POLICY rls_calendar_tasks ON calendar_tasks
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

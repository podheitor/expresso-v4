-- DOWN: calendar schema
BEGIN;

DROP TRIGGER IF EXISTS trg_calendars_touch      ON calendars;
DROP TRIGGER IF EXISTS trg_calendar_events_touch ON calendar_events;
DROP FUNCTION IF EXISTS calendars_touch();
DROP FUNCTION IF EXISTS calendar_events_touch();

DROP POLICY IF EXISTS rls_calendar_events ON calendar_events;
DROP POLICY IF EXISTS rls_calendars       ON calendars;
ALTER TABLE calendar_events DISABLE ROW LEVEL SECURITY;
ALTER TABLE calendars       DISABLE ROW LEVEL SECURITY;

DROP INDEX IF EXISTS idx_events_dtrange;
DROP INDEX IF EXISTS idx_events_calendar_dtstart;
DROP INDEX IF EXISTS idx_events_tenant;
DROP INDEX IF EXISTS uq_events_calendar_uid;
DROP TABLE IF EXISTS calendar_events;

DROP INDEX IF EXISTS uq_calendars_owner_default;
DROP INDEX IF EXISTS idx_calendars_owner;
DROP INDEX IF EXISTS idx_calendars_tenant;
DROP TABLE IF EXISTS calendars;

COMMIT;

-- DOWN: calendar event alarms table
BEGIN;
DROP INDEX IF EXISTS calendar_event_alarms_event_idx;
DROP TABLE IF EXISTS calendar_event_alarms;
COMMIT;

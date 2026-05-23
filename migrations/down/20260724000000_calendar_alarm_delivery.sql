-- DOWN: calendar alarm delivery index
BEGIN;
DROP INDEX IF EXISTS calendar_event_alarms_due_idx;
COMMIT;

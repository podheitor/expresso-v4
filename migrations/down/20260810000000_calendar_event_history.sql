-- DOWN: calendar event history
BEGIN;
DROP INDEX IF EXISTS idx_cal_event_history_event;
DROP TABLE IF EXISTS calendar_event_history;
COMMIT;

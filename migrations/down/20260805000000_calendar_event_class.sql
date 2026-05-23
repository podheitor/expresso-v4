-- DOWN: calendar event class column
BEGIN;
ALTER TABLE calendar_events DROP COLUMN IF EXISTS class;
COMMIT;

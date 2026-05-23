-- DOWN: calendar event transp column
BEGIN;
ALTER TABLE calendar_events DROP COLUMN IF EXISTS transp;
COMMIT;

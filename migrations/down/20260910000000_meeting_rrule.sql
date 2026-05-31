-- Revert: drop the meetings.rrule column.

BEGIN;

ALTER TABLE meetings DROP COLUMN IF EXISTS rrule;

COMMIT;

-- Revert: drop the recurring-task rrule column.

ALTER TABLE calendar_tasks DROP COLUMN IF EXISTS rrule;

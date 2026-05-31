-- Revert: drop the CalDAV roundtrip columns from calendar_tasks.

ALTER TABLE calendar_tasks
    DROP COLUMN IF EXISTS ical_raw,
    DROP COLUMN IF EXISTS etag;

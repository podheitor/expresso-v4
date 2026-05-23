-- DOWN: meetings recording column
BEGIN;
ALTER TABLE meetings DROP COLUMN IF EXISTS recording_started_at;
COMMIT;

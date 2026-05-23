-- DOWN: meeting recordings table
BEGIN;
DROP INDEX IF EXISTS meeting_recordings_meeting_idx;
DROP TABLE IF EXISTS meeting_recordings;
COMMIT;

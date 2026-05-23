-- DOWN: meeting transcripts
BEGIN;
DROP INDEX IF EXISTS meeting_transcripts_meeting_idx;
DROP TABLE IF EXISTS meeting_transcripts;
COMMIT;

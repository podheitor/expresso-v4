-- DOWN: meeting polls
BEGIN;
DROP INDEX IF EXISTS meeting_poll_votes_poll_idx;
DROP TABLE IF EXISTS meeting_poll_votes;
DROP INDEX IF EXISTS meeting_polls_meeting_idx;
DROP TABLE IF EXISTS meeting_polls;
COMMIT;

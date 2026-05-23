-- DOWN: meeting lobby
BEGIN;
DROP INDEX IF EXISTS meeting_lobby_meeting_idx;
DROP TABLE IF EXISTS meeting_lobby;
COMMIT;

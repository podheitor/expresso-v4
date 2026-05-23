-- DOWN: meeting breakout rooms
BEGIN;
DROP INDEX IF EXISTS meeting_breakout_participants_room_idx;
DROP TABLE IF EXISTS meeting_breakout_participants;
DROP INDEX IF EXISTS meeting_breakout_rooms_meeting_idx;
DROP TABLE IF EXISTS meeting_breakout_rooms;
COMMIT;

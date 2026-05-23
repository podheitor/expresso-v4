-- DOWN: meetings schema
BEGIN;

DROP TRIGGER IF EXISTS meetings_touch_trg ON meetings;
DROP FUNCTION IF EXISTS meetings_touch();

DROP INDEX IF EXISTS meeting_participants_tenant_user_idx;
DROP TABLE IF EXISTS meeting_participants;

DROP INDEX IF EXISTS meetings_sched_idx;
DROP INDEX IF EXISTS meetings_channel_idx;
DROP INDEX IF EXISTS meetings_tenant_idx;
DROP TABLE IF EXISTS meetings;

COMMIT;

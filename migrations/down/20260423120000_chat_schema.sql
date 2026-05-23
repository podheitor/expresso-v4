-- DOWN: chat schema
BEGIN;

DROP TRIGGER IF EXISTS chat_channels_touch_trg ON chat_channels;
DROP FUNCTION IF EXISTS chat_channels_touch();

ALTER TABLE chat_channel_members DISABLE ROW LEVEL SECURITY;
ALTER TABLE chat_channels        DISABLE ROW LEVEL SECURITY;

DROP INDEX IF EXISTS chat_members_tenant_user_idx;
DROP TABLE IF EXISTS chat_channel_members;

DROP INDEX IF EXISTS chat_channels_team_idx;
DROP INDEX IF EXISTS chat_channels_tenant_idx;
DROP TABLE IF EXISTS chat_channels;

COMMIT;

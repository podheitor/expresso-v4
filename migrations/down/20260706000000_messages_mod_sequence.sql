-- DOWN: messages mod_sequence
BEGIN;
DROP TRIGGER IF EXISTS trig_messages_mod_sequence ON messages;
DROP FUNCTION IF EXISTS messages_bump_mod_sequence();
DROP INDEX IF EXISTS idx_messages_mod_sequence;
ALTER TABLE messages DROP COLUMN IF EXISTS mod_sequence;
COMMIT;

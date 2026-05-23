-- DOWN: scheduled send
BEGIN;
DROP INDEX IF EXISTS messages_deliver_at_idx;
ALTER TABLE messages DROP COLUMN IF EXISTS deliver_at;
COMMIT;

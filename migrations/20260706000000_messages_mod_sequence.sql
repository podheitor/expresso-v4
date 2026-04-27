-- Add mod_sequence to messages for IMAP CONDSTORE (RFC 7162).
-- A global sequence ensures mod_sequence values are strictly increasing
-- across all messages in the database, satisfying RFC 7162 §3.

BEGIN;

CREATE SEQUENCE IF NOT EXISTS messages_modseq_seq START 1;

ALTER TABLE messages
    ADD COLUMN IF NOT EXISTS mod_sequence BIGINT NOT NULL DEFAULT nextval('messages_modseq_seq');

-- Backfill existing rows with unique ascending values (oldest first).
-- Each row gets a distinct value from the sequence.
UPDATE messages m
   SET mod_sequence = nextval('messages_modseq_seq')
  FROM (
      SELECT id FROM messages ORDER BY received_at ASC
  ) ordered
 WHERE m.id = ordered.id;

-- Trigger: bump mod_sequence on every UPDATE (flag change, move, etc.).
CREATE OR REPLACE FUNCTION messages_bump_mod_sequence()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.mod_sequence = nextval('messages_modseq_seq');
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trig_messages_mod_sequence ON messages;
CREATE TRIGGER trig_messages_mod_sequence
    BEFORE UPDATE ON messages
    FOR EACH ROW EXECUTE FUNCTION messages_bump_mod_sequence();

-- Index for efficient CHANGEDSINCE queries.
CREATE INDEX IF NOT EXISTS idx_messages_mod_sequence
    ON messages (mailbox_id, tenant_id, mod_sequence);

COMMIT;

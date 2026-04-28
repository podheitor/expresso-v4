ALTER TABLE messages ADD COLUMN IF NOT EXISTS deliver_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS messages_deliver_at_idx
    ON messages (deliver_at)
    WHERE deliver_at IS NOT NULL;

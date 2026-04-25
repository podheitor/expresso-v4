-- RFC 3501 §6.3.6-8: SUBSCRIBE/UNSUBSCRIBE/LSUB
-- Clients mark folders they want to see in their folder list.
-- Default TRUE so existing mailboxes are immediately visible via LSUB.
ALTER TABLE mailboxes
    ADD COLUMN IF NOT EXISTS subscribed BOOLEAN NOT NULL DEFAULT TRUE;

CREATE INDEX IF NOT EXISTS idx_mailboxes_subscribed
    ON mailboxes (user_id, tenant_id, subscribed);

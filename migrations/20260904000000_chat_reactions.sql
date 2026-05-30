-- Migration: emoji reactions on chat messages.
-- One row per (message, user, emoji): a user may react with several distinct
-- emojis to one message but not duplicate the same emoji. event_id is the
-- Matrix event id of the target message (messages live in Matrix; this table
-- is the expresso-side reaction index for fast per-message aggregation).

BEGIN;

CREATE TABLE IF NOT EXISTS chat_reactions (
    channel_id  UUID        NOT NULL REFERENCES chat_channels (id) ON DELETE CASCADE,
    tenant_id   UUID        NOT NULL,
    event_id    TEXT        NOT NULL,
    user_id     UUID        NOT NULL,
    emoji       TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (channel_id, event_id, user_id, emoji)
);

-- Aggregation lookup: "all reactions for these messages in this channel".
CREATE INDEX IF NOT EXISTS chat_reactions_channel_event_idx
    ON chat_reactions (channel_id, event_id);

-- RLS, mirroring the other chat tables.
ALTER TABLE chat_reactions ENABLE ROW LEVEL SECURITY;

CREATE POLICY chat_reactions_isolation ON chat_reactions
    USING (current_setting('app.tenant_id', true) IS NULL
           OR tenant_id = current_setting('app.tenant_id', true)::uuid);

COMMIT;

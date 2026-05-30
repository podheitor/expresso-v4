-- Migration: file/image/video/audio attachments on chat messages.
-- One row per uploaded attachment. The bytes live in the Matrix media repo
-- (mxc_uri); this table is the expresso-side index so a channel's attachments
-- can be listed without scanning Matrix message history. event_id is the
-- Matrix event id of the m.room.message that references the upload.

BEGIN;

CREATE TABLE IF NOT EXISTS chat_attachments (
    id            UUID        NOT NULL DEFAULT gen_random_uuid(),
    channel_id    UUID        NOT NULL REFERENCES chat_channels (id) ON DELETE CASCADE,
    tenant_id     UUID        NOT NULL,
    user_id       UUID        NOT NULL,
    event_id      TEXT        NOT NULL,
    filename      TEXT        NOT NULL,
    content_type  TEXT        NOT NULL,
    size_bytes    BIGINT      NOT NULL,
    kind          TEXT        NOT NULL,
    mxc_uri       TEXT        NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id)
);

-- Listing lookup: "all attachments in this channel, newest first".
CREATE INDEX IF NOT EXISTS chat_attachments_channel_idx
    ON chat_attachments (channel_id, created_at DESC);

-- RLS, mirroring the other chat tables.
ALTER TABLE chat_attachments ENABLE ROW LEVEL SECURITY;

CREATE POLICY chat_attachments_isolation ON chat_attachments
    USING (current_setting('app.tenant_id', true) IS NULL
           OR tenant_id = current_setting('app.tenant_id', true)::uuid);

COMMIT;

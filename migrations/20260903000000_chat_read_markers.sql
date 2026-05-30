-- Migration: per-user, per-channel read markers for unread tracking.
-- A sparse overlay on chat_channel_members: a row records the last time a user
-- read a channel. "Unread" is then derived as chat_channels.updated_at (bumped
-- on each message send) > last_read_at — a boolean status, not a message count
-- (chat messages live in Matrix, not this DB, so counting would need an HS
-- round-trip per channel). Absent row ⇒ never read ⇒ unread if the channel has
-- any activity.

BEGIN;

CREATE TABLE IF NOT EXISTS chat_read_markers (
    channel_id   UUID        NOT NULL REFERENCES chat_channels (id) ON DELETE CASCADE,
    tenant_id    UUID        NOT NULL,
    user_id      UUID        NOT NULL,
    last_read_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (channel_id, user_id)
);

CREATE INDEX IF NOT EXISTS chat_read_markers_tenant_user_idx
    ON chat_read_markers (tenant_id, user_id);

-- RLS, mirroring chat_channels / chat_channel_members: bypass during bootstrap
-- (app.tenant_id unset) or match the current tenant setting.
ALTER TABLE chat_read_markers ENABLE ROW LEVEL SECURITY;

CREATE POLICY chat_read_markers_isolation ON chat_read_markers
    USING (current_setting('app.tenant_id', true) IS NULL
           OR tenant_id = current_setting('app.tenant_id', true)::uuid);

COMMIT;

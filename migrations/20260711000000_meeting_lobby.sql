CREATE TABLE IF NOT EXISTS meeting_lobby (
    meeting_id  UUID NOT NULL,
    tenant_id   UUID NOT NULL,
    user_id     UUID NOT NULL,
    joined_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (meeting_id, user_id)
);

CREATE INDEX IF NOT EXISTS meeting_lobby_meeting_idx
    ON meeting_lobby (tenant_id, meeting_id, joined_at);

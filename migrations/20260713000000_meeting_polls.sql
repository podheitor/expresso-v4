CREATE TABLE IF NOT EXISTS meeting_polls (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id  UUID NOT NULL,
    tenant_id   UUID NOT NULL,
    created_by  UUID NOT NULL,
    question    TEXT NOT NULL,
    options     JSONB NOT NULL DEFAULT '[]',
    is_closed   BOOLEAN NOT NULL DEFAULT false,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS meeting_polls_meeting_idx
    ON meeting_polls (tenant_id, meeting_id, created_at);

CREATE TABLE IF NOT EXISTS meeting_poll_votes (
    poll_id    UUID NOT NULL,
    tenant_id  UUID NOT NULL,
    user_id    UUID NOT NULL,
    option_idx INTEGER NOT NULL,
    voted_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (poll_id, user_id)
);

CREATE INDEX IF NOT EXISTS meeting_poll_votes_poll_idx
    ON meeting_poll_votes (poll_id, option_idx);

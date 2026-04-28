-- Sprint #404: meeting transcript metadata
CREATE TABLE IF NOT EXISTS meeting_transcripts (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id  UUID NOT NULL,
    tenant_id   UUID NOT NULL,
    url         TEXT NOT NULL,
    language    TEXT,
    starts_at   TIMESTAMPTZ,
    ends_at     TIMESTAMPTZ,
    created_by  UUID NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (meeting_id, url)
);

CREATE INDEX IF NOT EXISTS meeting_transcripts_meeting_idx
    ON meeting_transcripts (tenant_id, meeting_id);

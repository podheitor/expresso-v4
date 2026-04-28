-- Sprint #413: meeting recording metadata
CREATE TABLE IF NOT EXISTS meeting_recordings (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id  UUID NOT NULL,
    tenant_id   UUID NOT NULL,
    url         TEXT NOT NULL,
    duration_s  INTEGER,                 -- duration in seconds, nullable until known
    size_bytes  BIGINT,
    format      TEXT,                    -- e.g. mp4, webm
    starts_at   TIMESTAMPTZ,
    ends_at     TIMESTAMPTZ,
    created_by  UUID NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (meeting_id, url)
);

CREATE INDEX IF NOT EXISTS meeting_recordings_meeting_idx
    ON meeting_recordings (tenant_id, meeting_id);

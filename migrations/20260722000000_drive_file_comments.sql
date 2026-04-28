-- Sprint #405: drive file comments
CREATE TABLE IF NOT EXISTS drive_file_comments (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    file_id     UUID NOT NULL,
    tenant_id   UUID NOT NULL,
    user_id     UUID NOT NULL,
    body        TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS drive_file_comments_file_idx
    ON drive_file_comments (tenant_id, file_id, created_at DESC);

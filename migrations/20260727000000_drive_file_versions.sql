-- Sprint #411: drive file version history
CREATE TABLE IF NOT EXISTS drive_file_versions (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    file_id     UUID NOT NULL,
    tenant_id   UUID NOT NULL,
    version_num BIGINT NOT NULL,           -- monotonically increasing per file
    size_bytes  BIGINT NOT NULL DEFAULT 0,
    sha256      TEXT,
    storage_key TEXT,
    created_by  UUID NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (file_id, version_num)
);

CREATE INDEX IF NOT EXISTS drive_file_versions_file_idx
    ON drive_file_versions (tenant_id, file_id, version_num DESC);

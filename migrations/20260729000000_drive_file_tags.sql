-- Sprint #415: drive file tags
CREATE TABLE IF NOT EXISTS drive_file_tags (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    file_id     UUID NOT NULL,
    tenant_id   UUID NOT NULL,
    tag         TEXT NOT NULL CHECK (char_length(tag) BETWEEN 1 AND 64),
    created_by  UUID NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (file_id, tenant_id, tag)
);

CREATE INDEX IF NOT EXISTS drive_file_tags_file_idx
    ON drive_file_tags (tenant_id, file_id);

CREATE INDEX IF NOT EXISTS drive_file_tags_tag_idx
    ON drive_file_tags (tenant_id, tag);

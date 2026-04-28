CREATE TABLE IF NOT EXISTS drive_file_tags (
    tenant_id UUID        NOT NULL,
    file_id   UUID        NOT NULL REFERENCES drive_files(id) ON DELETE CASCADE,
    tag       TEXT        NOT NULL CHECK (char_length(tag) BETWEEN 1 AND 64),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, file_id, tag)
);

CREATE INDEX IF NOT EXISTS drive_file_tags_file
    ON drive_file_tags (tenant_id, file_id);

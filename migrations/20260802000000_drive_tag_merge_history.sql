-- Sprint #477: drive tag merge history (enables undo + audit)
CREATE TABLE IF NOT EXISTS drive_tag_merge_history (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id          UUID NOT NULL,
    src_tag            TEXT NOT NULL CHECK (char_length(src_tag) BETWEEN 1 AND 64),
    dst_tag            TEXT NOT NULL CHECK (char_length(dst_tag) BETWEEN 1 AND 64),
    merged_count       BIGINT NOT NULL,
    merged_file_ids    UUID[] NOT NULL DEFAULT '{}',
    dropped_file_ids   UUID[] NOT NULL DEFAULT '{}',
    merged_by          UUID NOT NULL,
    merged_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS drive_tag_merge_history_tenant_at_idx
    ON drive_tag_merge_history (tenant_id, merged_at DESC);

CREATE INDEX IF NOT EXISTS drive_tag_merge_history_tag_idx
    ON drive_tag_merge_history (tenant_id, src_tag);

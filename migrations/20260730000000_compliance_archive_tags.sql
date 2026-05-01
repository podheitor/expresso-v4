-- Sprint #460: compliance archive entry tags
CREATE TABLE IF NOT EXISTS compliance_archive_tags (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    archive_id  UUID NOT NULL,
    tenant_id   UUID NOT NULL,
    tag         TEXT NOT NULL CHECK (char_length(tag) BETWEEN 1 AND 64),
    created_by  UUID NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (archive_id, tenant_id, tag)
);

CREATE INDEX IF NOT EXISTS compliance_archive_tags_entry_idx
    ON compliance_archive_tags (tenant_id, archive_id);

CREATE INDEX IF NOT EXISTS compliance_archive_tags_tag_idx
    ON compliance_archive_tags (tenant_id, tag);

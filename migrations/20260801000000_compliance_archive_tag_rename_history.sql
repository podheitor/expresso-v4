-- Sprint #475: compliance archive tag rename history
CREATE TABLE IF NOT EXISTS compliance_archive_tag_rename_history (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL,
    user_id         UUID NOT NULL,
    old_tag         TEXT NOT NULL CHECK (char_length(old_tag) BETWEEN 1 AND 64),
    new_tag         TEXT NOT NULL CHECK (char_length(new_tag) BETWEEN 1 AND 64),
    renamed_count   BIGINT NOT NULL,
    renamed_by      UUID NOT NULL,
    renamed_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS compliance_archive_tag_rename_history_tenant_at_idx
    ON compliance_archive_tag_rename_history (tenant_id, renamed_at DESC);

CREATE INDEX IF NOT EXISTS compliance_archive_tag_rename_history_tag_idx
    ON compliance_archive_tag_rename_history (tenant_id, old_tag);

-- Sprint #571: folder-level quota para drive.
-- Quota configurável por pasta (folder); enforced no upload.
-- NULL folder_id não é permitido — quota de tenant está em drive_quotas.
-- used_bytes é computado on-demand via SUM(size_bytes) shallow (filhos diretos).

CREATE TABLE IF NOT EXISTS drive_folder_quotas (
    tenant_id  UUID        NOT NULL,
    folder_id  UUID        NOT NULL REFERENCES drive_files(id) ON DELETE CASCADE,
    max_bytes  BIGINT      NOT NULL CHECK (max_bytes > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, folder_id)
);

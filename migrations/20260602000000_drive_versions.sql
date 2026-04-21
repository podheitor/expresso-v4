-- Drive file versioning (Phase 3).
--
-- Cada upload de arquivo com (parent_id, name) pré-existente arquiva o
-- conteúdo atual em drive_file_versions antes de substituir a linha viva
-- em drive_files com novo storage_key/sha256/size. Histórico preserva o
-- blob no disco sob mesmo layout flat (<data_root>/<storage_key>).

BEGIN;

CREATE TABLE IF NOT EXISTS drive_file_versions (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    file_id     UUID        NOT NULL REFERENCES drive_files(id) ON DELETE CASCADE,
    tenant_id   UUID        NOT NULL REFERENCES tenants(id)     ON DELETE CASCADE,
    version_no  INTEGER     NOT NULL CHECK (version_no >= 1),
    storage_key TEXT        NOT NULL,
    size_bytes  BIGINT      NOT NULL DEFAULT 0,
    sha256      TEXT,
    mime_type   TEXT,
    created_by  UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (file_id, version_no)
);

CREATE INDEX IF NOT EXISTS idx_drive_file_versions_tenant ON drive_file_versions (tenant_id);
CREATE INDEX IF NOT EXISTS idx_drive_file_versions_file   ON drive_file_versions (file_id);

ALTER TABLE drive_file_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE drive_file_versions FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_drive_file_versions ON drive_file_versions;
CREATE POLICY rls_drive_file_versions ON drive_file_versions
    USING      (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

COMMIT;

-- Expresso Drive (Phase 3) — file metadata + quotas, content on disk/S3.
--
-- Phase 3 scaffold: files are persisted in the service's data directory
-- keyed by storage_key (UUID). DB holds metadata only; a later migration
-- will add sharing/versioning/ACL tables.

BEGIN;

CREATE TABLE IF NOT EXISTS drive_files (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    owner_user_id UUID        NOT NULL REFERENCES users(id)   ON DELETE CASCADE,
    parent_id     UUID                 REFERENCES drive_files(id) ON DELETE CASCADE,
    name          TEXT        NOT NULL,
    kind          TEXT        NOT NULL CHECK (kind IN ('file','folder')),
    mime_type     TEXT,
    size_bytes    BIGINT      NOT NULL DEFAULT 0,
    sha256        TEXT,
    storage_key   TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at    TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_drive_files_tenant   ON drive_files (tenant_id);
CREATE INDEX IF NOT EXISTS idx_drive_files_owner    ON drive_files (owner_user_id);
CREATE INDEX IF NOT EXISTS idx_drive_files_parent   ON drive_files (parent_id);
CREATE UNIQUE INDEX IF NOT EXISTS uq_drive_files_sibling_name
    ON drive_files (tenant_id, COALESCE(parent_id, '00000000-0000-0000-0000-000000000000'), name)
    WHERE deleted_at IS NULL;

ALTER TABLE drive_files ENABLE ROW LEVEL SECURITY;
ALTER TABLE drive_files FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_drive_files ON drive_files;
CREATE POLICY rls_drive_files ON drive_files
    USING       (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK  (tenant_id = current_setting('app.tenant_id', true)::uuid);

COMMIT;

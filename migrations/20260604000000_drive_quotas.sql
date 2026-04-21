-- Drive per-tenant quotas (Phase 3).
--
-- max_bytes persistido por tenant; used_bytes calculado on-demand via
-- agregação (drive_files vivos + histórico de versions). Trash não conta
-- → obriga purge p/ liberar espaço. Default 10 GB se linha ausente.

BEGIN;

CREATE TABLE IF NOT EXISTS drive_quotas (
    tenant_id  UUID        PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    max_bytes  BIGINT      NOT NULL CHECK (max_bytes >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE drive_quotas ENABLE ROW LEVEL SECURITY;
ALTER TABLE drive_quotas FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_drive_quotas ON drive_quotas;
CREATE POLICY rls_drive_quotas ON drive_quotas
    USING      (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

-- Uso corrente (bytes) por tenant — soma arquivos vivos + versões.
CREATE OR REPLACE FUNCTION drive_quota_used(p_tenant UUID)
RETURNS BIGINT
LANGUAGE sql
STABLE
AS $$
    SELECT COALESCE((
        SELECT SUM(size_bytes)::BIGINT FROM drive_files
         WHERE tenant_id = p_tenant AND deleted_at IS NULL AND kind = 'file'
    ), 0)
    + COALESCE((
        SELECT SUM(size_bytes)::BIGINT FROM drive_file_versions
         WHERE tenant_id = p_tenant
    ), 0)
$$;

COMMIT;

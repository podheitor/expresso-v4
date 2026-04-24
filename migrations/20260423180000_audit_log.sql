-- Audit log: cross-service append-only trail of actor actions.
-- Idempotent: if table pre-exists with older schema, ALTER ensures new cols.

CREATE TABLE IF NOT EXISTS audit_log (
    id            BIGSERIAL PRIMARY KEY,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS tenant_id    UUID;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS actor_sub    TEXT;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS actor_email  TEXT;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS actor_roles  TEXT[] NOT NULL DEFAULT '{}';
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS action       TEXT NOT NULL DEFAULT '';
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS target_type  TEXT;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS target_id    TEXT;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS http_method  TEXT;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS http_path    TEXT;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS status_code  SMALLINT;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS metadata     JSONB NOT NULL DEFAULT '{}'::jsonb;

CREATE INDEX IF NOT EXISTS audit_log_created_idx    ON audit_log (created_at DESC);
CREATE INDEX IF NOT EXISTS audit_log_tenant_idx     ON audit_log (tenant_id, created_at DESC) WHERE tenant_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS audit_log_actor_idx      ON audit_log (actor_sub, created_at DESC) WHERE actor_sub IS NOT NULL;
CREATE INDEX IF NOT EXISTS audit_log_action_idx     ON audit_log (action, created_at DESC);

COMMENT ON TABLE  audit_log IS 'Cross-service audit trail (SuperAdmin + iMIP + DAV mutations).';
COMMENT ON COLUMN audit_log.action IS 'Dotted identifier, e.g. admin.tenant.create, admin.user.delete, imip.cancel';

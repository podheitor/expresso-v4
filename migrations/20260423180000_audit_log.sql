-- Audit log: cross-service append-only trail of actor actions.
-- Generic schema: caller sets action/target/metadata; middleware/handlers
-- populate actor (sub/email/roles) + HTTP envelope when applicable.
-- Append-only: no UPDATE/DELETE from app; tombstone GC may purge by age.

CREATE TABLE IF NOT EXISTS audit_log (
    id            BIGSERIAL PRIMARY KEY,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    tenant_id     UUID,
    actor_sub     TEXT,
    actor_email   TEXT,
    actor_roles   TEXT[] NOT NULL DEFAULT '{}',
    action        TEXT NOT NULL,
    target_type   TEXT,
    target_id     TEXT,
    http_method   TEXT,
    http_path     TEXT,
    status_code   SMALLINT,
    metadata      JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS audit_log_created_idx    ON audit_log (created_at DESC);
CREATE INDEX IF NOT EXISTS audit_log_tenant_idx     ON audit_log (tenant_id, created_at DESC) WHERE tenant_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS audit_log_actor_idx      ON audit_log (actor_sub, created_at DESC) WHERE actor_sub IS NOT NULL;
CREATE INDEX IF NOT EXISTS audit_log_action_idx     ON audit_log (action, created_at DESC);

COMMENT ON TABLE  audit_log IS 'Cross-service audit trail (SuperAdmin + iMIP + DAV mutations).';
COMMENT ON COLUMN audit_log.action IS 'Dotted identifier, e.g. admin.tenant.create, admin.user.delete, imip.cancel';

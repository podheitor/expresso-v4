-- DOWN: audit_log v2 columns and indexes
BEGIN;

DROP INDEX IF EXISTS audit_log_action_idx;
DROP INDEX IF EXISTS audit_log_actor_idx;
DROP INDEX IF EXISTS audit_log_tenant_idx;
DROP INDEX IF EXISTS audit_log_created_idx;

ALTER TABLE audit_log
    DROP COLUMN IF EXISTS metadata,
    DROP COLUMN IF EXISTS status_code,
    DROP COLUMN IF EXISTS http_path,
    DROP COLUMN IF EXISTS http_method,
    DROP COLUMN IF EXISTS target_id,
    DROP COLUMN IF EXISTS target_type,
    DROP COLUMN IF EXISTS action,
    DROP COLUMN IF EXISTS actor_roles,
    DROP COLUMN IF EXISTS actor_email,
    DROP COLUMN IF EXISTS actor_sub,
    DROP COLUMN IF EXISTS tenant_id;

COMMIT;

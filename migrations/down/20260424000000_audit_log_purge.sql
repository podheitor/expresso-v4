-- DOWN: audit_log_purge function
BEGIN;
DROP FUNCTION IF EXISTS audit_log_purge(INT);
COMMIT;

-- DOWN: calendar ACL
BEGIN;
DROP INDEX IF EXISTS idx_calendar_acl_tenant;
DROP INDEX IF EXISTS idx_calendar_acl_grantee;
DROP TABLE IF EXISTS calendar_acl;
COMMIT;

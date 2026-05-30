-- DOWN: drive file ACL
BEGIN;

DROP POLICY IF EXISTS rls_drive_file_acl ON drive_file_acl;
ALTER TABLE drive_file_acl DISABLE ROW LEVEL SECURITY;

DROP INDEX IF EXISTS idx_drive_file_acl_tenant;
DROP INDEX IF EXISTS idx_drive_file_acl_grantee;
DROP TABLE IF EXISTS drive_file_acl;

COMMIT;

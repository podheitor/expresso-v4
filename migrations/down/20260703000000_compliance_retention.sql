-- DOWN: compliance retention
BEGIN;
DROP INDEX IF EXISTS compliance_archive_tenant_user;
DROP TABLE IF EXISTS compliance_archive;
DROP INDEX IF EXISTS retention_policies_tenant;
DROP TABLE IF EXISTS retention_policies;
COMMIT;

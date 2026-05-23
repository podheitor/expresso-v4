-- DOWN: compliance archive tag rename history
BEGIN;
DROP INDEX IF EXISTS compliance_archive_tag_rename_history_tag_idx;
DROP INDEX IF EXISTS compliance_archive_tag_rename_history_tenant_at_idx;
DROP TABLE IF EXISTS compliance_archive_tag_rename_history;
COMMIT;

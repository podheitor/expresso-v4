-- DOWN: compliance archive tag merge history
BEGIN;
DROP INDEX IF EXISTS compliance_archive_tag_merge_history_src_idx;
DROP INDEX IF EXISTS compliance_archive_tag_merge_history_tenant_user_at_idx;
DROP TABLE IF EXISTS compliance_archive_tag_merge_history;
COMMIT;

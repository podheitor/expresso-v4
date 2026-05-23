-- DOWN: compliance archive tags
BEGIN;
DROP INDEX IF EXISTS compliance_archive_tags_tag_idx;
DROP INDEX IF EXISTS compliance_archive_tags_entry_idx;
DROP TABLE IF EXISTS compliance_archive_tags;
COMMIT;

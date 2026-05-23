-- DOWN: drive tag merge history
BEGIN;
DROP INDEX IF EXISTS drive_tag_merge_history_tag_idx;
DROP INDEX IF EXISTS drive_tag_merge_history_tenant_at_idx;
DROP TABLE IF EXISTS drive_tag_merge_history;
COMMIT;

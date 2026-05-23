-- DOWN: drive tag rename history
BEGIN;
DROP INDEX IF EXISTS drive_tag_rename_history_tag_idx;
DROP INDEX IF EXISTS drive_tag_rename_history_tenant_at_idx;
DROP TABLE IF EXISTS drive_tag_rename_history;
COMMIT;

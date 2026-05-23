-- DOWN: drive file tags v2 (with tag_idx)
BEGIN;
DROP INDEX IF EXISTS drive_file_tags_tag_idx;
DROP INDEX IF EXISTS drive_file_tags_file_idx;
DROP TABLE IF EXISTS drive_file_tags;
COMMIT;

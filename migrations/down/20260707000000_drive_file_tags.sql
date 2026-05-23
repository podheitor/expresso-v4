-- DOWN: drive file tags (first version)
BEGIN;
DROP INDEX IF EXISTS drive_file_tags_file;
DROP TABLE IF EXISTS drive_file_tags;
COMMIT;

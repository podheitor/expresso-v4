-- DOWN: drive file comments
BEGIN;
DROP INDEX IF EXISTS drive_file_comments_file_idx;
DROP TABLE IF EXISTS drive_file_comments;
COMMIT;

-- DOWN: drive file activity
BEGIN;
DROP INDEX IF EXISTS drive_file_activity_user_idx;
DROP INDEX IF EXISTS drive_file_activity_file_idx;
DROP TABLE IF EXISTS drive_file_activity;
COMMIT;

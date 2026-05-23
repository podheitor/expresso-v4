-- DOWN: drive file starred
BEGIN;
DROP INDEX IF EXISTS drive_files_starred_idx;
ALTER TABLE drive_files DROP COLUMN IF EXISTS starred_at;
COMMIT;

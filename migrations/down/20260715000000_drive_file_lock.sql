-- DOWN: drive file lock columns
BEGIN;
DROP INDEX IF EXISTS drive_files_locked_idx;
ALTER TABLE drive_files DROP COLUMN IF EXISTS locked_at;
ALTER TABLE drive_files DROP COLUMN IF EXISTS locked_by;
COMMIT;

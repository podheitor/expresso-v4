-- DOWN: drive file expiry
BEGIN;
DROP INDEX IF EXISTS drive_files_expires_at_idx;
ALTER TABLE drive_files DROP COLUMN IF EXISTS expires_at;
COMMIT;

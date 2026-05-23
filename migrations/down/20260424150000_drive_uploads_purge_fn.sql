-- DOWN: drive uploads purge function
BEGIN;
DROP FUNCTION IF EXISTS drive_uploads_purge_expired();
COMMIT;

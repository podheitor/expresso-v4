-- DOWN: drive uploads (tus)
BEGIN;
DROP INDEX IF EXISTS ix_drive_uploads_expires;
DROP INDEX IF EXISTS ix_drive_uploads_tenant;
DROP TABLE IF EXISTS drive_uploads;
COMMIT;

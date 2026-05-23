-- DOWN: drive WOPI locks
BEGIN;
DROP INDEX IF EXISTS ix_drive_wopi_locks_expires;
DROP INDEX IF EXISTS ix_drive_wopi_locks_tenant;
DROP TABLE IF EXISTS drive_wopi_locks;
COMMIT;

-- DOWN: drive schema
BEGIN;
DROP INDEX IF EXISTS uq_drive_files_sibling_name;
DROP INDEX IF EXISTS idx_drive_files_parent;
DROP INDEX IF EXISTS idx_drive_files_owner;
DROP INDEX IF EXISTS idx_drive_files_tenant;
DROP TABLE IF EXISTS drive_files;
COMMIT;

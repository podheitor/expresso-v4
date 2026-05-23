-- DOWN: drive file versions (first table)
BEGIN;
DROP INDEX IF EXISTS idx_drive_file_versions_file;
DROP INDEX IF EXISTS idx_drive_file_versions_tenant;
DROP TABLE IF EXISTS drive_file_versions;
COMMIT;

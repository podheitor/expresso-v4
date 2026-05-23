-- DOWN: drive file versions (second table — supersedes 20260602)
BEGIN;
DROP INDEX IF EXISTS drive_file_versions_file_idx;
DROP TABLE IF EXISTS drive_file_versions;
COMMIT;

-- DOWN: drive quotas
BEGIN;
DROP FUNCTION IF EXISTS drive_quota_used(UUID);
DROP TABLE IF EXISTS drive_quotas;
COMMIT;

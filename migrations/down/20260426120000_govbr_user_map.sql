-- DOWN: govbr user map
BEGIN;
DROP INDEX IF EXISTS idx_govbr_user_map_user;
DROP INDEX IF EXISTS idx_govbr_user_map_tenant;
DROP TABLE IF EXISTS govbr_user_map;
COMMIT;

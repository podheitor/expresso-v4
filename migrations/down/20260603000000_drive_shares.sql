-- DOWN: drive shares
BEGIN;
DROP FUNCTION IF EXISTS drive_share_resolve(TEXT);
DROP INDEX IF EXISTS idx_drive_shares_file;
DROP INDEX IF EXISTS idx_drive_shares_tenant;
DROP TABLE IF EXISTS drive_shares;
COMMIT;

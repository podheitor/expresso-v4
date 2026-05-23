-- DOWN: user sieve
BEGIN;
DROP INDEX IF EXISTS idx_user_sieve_tenant;
DROP TABLE IF EXISTS user_sieve;
COMMIT;

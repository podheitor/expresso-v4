-- DOWN: per-user email signatures
BEGIN;
DROP INDEX IF EXISTS user_signatures_one_default_idx;
DROP INDEX IF EXISTS user_signatures_user_idx;
DROP TABLE IF EXISTS user_signatures;
COMMIT;

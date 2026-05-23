-- DOWN: user vacation deactivate_at column
BEGIN;
ALTER TABLE user_vacation DROP COLUMN IF EXISTS deactivate_at;
COMMIT;

-- DOWN: govbr_user_map updated_at trigger
BEGIN;
DROP TRIGGER IF EXISTS trig_govbr_user_map_updated_at ON govbr_user_map;
DROP FUNCTION IF EXISTS govbr_user_map_set_updated_at();
ALTER TABLE govbr_user_map DROP COLUMN IF EXISTS updated_at;
COMMIT;

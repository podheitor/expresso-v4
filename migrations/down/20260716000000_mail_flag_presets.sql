-- DOWN: mail flag presets
BEGIN;
DROP INDEX IF EXISTS mail_flag_presets_user_idx;
DROP TABLE IF EXISTS mail_flag_presets;
COMMIT;

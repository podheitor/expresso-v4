-- DOWN: mail snooze
BEGIN;
DROP INDEX IF EXISTS mail_snooze_user_idx;
DROP INDEX IF EXISTS mail_snooze_due_idx;
DROP TABLE IF EXISTS mail_snooze;
COMMIT;

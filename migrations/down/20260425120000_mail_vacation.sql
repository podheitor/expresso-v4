-- DOWN: mail vacation
BEGIN;
DROP INDEX IF EXISTS idx_user_vacation_tenant;
DROP TABLE IF EXISTS user_vacation;
COMMIT;

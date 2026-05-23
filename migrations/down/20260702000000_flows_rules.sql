-- DOWN: flow rules
BEGIN;
DROP INDEX IF EXISTS flow_rules_user_tenant;
DROP TABLE IF EXISTS flow_rules;
COMMIT;

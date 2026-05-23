-- DOWN: DAV dead properties
BEGIN;
DROP INDEX IF EXISTS ix_ab_dead_prop_tenant;
DROP TABLE IF EXISTS addressbook_dead_properties;
DROP INDEX IF EXISTS ix_cal_dead_prop_tenant;
DROP TABLE IF EXISTS calendar_dead_properties;
COMMIT;

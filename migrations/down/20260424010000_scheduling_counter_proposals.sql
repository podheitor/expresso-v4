-- DOWN: scheduling counter proposals
BEGIN;
DROP INDEX IF EXISTS idx_counter_prop_event;
DROP INDEX IF EXISTS idx_counter_prop_tenant_status;
DROP TABLE IF EXISTS scheduling_counter_proposals;
COMMIT;

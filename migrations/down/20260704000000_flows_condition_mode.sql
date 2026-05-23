-- DOWN: flows condition mode
-- UP added a column with DEFAULT; reverse by dropping it.
BEGIN;
ALTER TABLE flow_rules DROP COLUMN IF EXISTS condition_mode;
COMMIT;

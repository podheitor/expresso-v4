-- DOWN: tombstones purge function
BEGIN;
DROP FUNCTION IF EXISTS tombstones_purge(INT);
COMMIT;

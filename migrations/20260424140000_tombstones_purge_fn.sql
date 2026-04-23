-- Tombstones retention function (mirrors audit_log_purge pattern).
-- Purges rows older than `retention_days` from both CalDAV/CardDAV tombstone tables.
-- Usage: SELECT tombstones_purge(30)
--   → returns total rows deleted across calendar_event_tombstones + contact_tombstones.

CREATE OR REPLACE FUNCTION tombstones_purge(retention_days INT)
RETURNS BIGINT
LANGUAGE plpgsql
AS $$
DECLARE
    cutoff    TIMESTAMPTZ;
    total     BIGINT := 0;
    batch     BIGINT;
BEGIN
    IF retention_days IS NULL OR retention_days < 1 THEN
        RAISE EXCEPTION 'tombstones_purge: retention_days must be >= 1 (got %)', retention_days;
    END IF;

    cutoff := NOW() - (retention_days || ' days')::INTERVAL;

    LOOP
        DELETE FROM calendar_event_tombstones
         WHERE ctid IN (
             SELECT ctid FROM calendar_event_tombstones
              WHERE deleted_at < cutoff
              FOR UPDATE SKIP LOCKED
              LIMIT 5000
         );
        GET DIAGNOSTICS batch = ROW_COUNT;
        total := total + batch;
        EXIT WHEN batch = 0;
    END LOOP;

    LOOP
        DELETE FROM contact_tombstones
         WHERE ctid IN (
             SELECT ctid FROM contact_tombstones
              WHERE deleted_at < cutoff
              FOR UPDATE SKIP LOCKED
              LIMIT 5000
         );
        GET DIAGNOSTICS batch = ROW_COUNT;
        total := total + batch;
        EXIT WHEN batch = 0;
    END LOOP;

    RETURN total;
END;
$$;

COMMENT ON FUNCTION tombstones_purge(INT) IS
  'Purge CalDAV/CardDAV tombstones older than retention_days (batched 5000 rows).';

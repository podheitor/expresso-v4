-- Audit log retention function.
-- Usage: SELECT audit_log_purge(90);  -- deletes rows older than 90 days, returns count.
--
-- Idempotent: re-running replaces function. Uses row-level DELETE (not TRUNCATE)
-- so triggers/replication continue working. Batches via DELETE ... WHERE id IN
-- to avoid long locks on large tables.

CREATE OR REPLACE FUNCTION audit_log_purge(retention_days INT)
RETURNS BIGINT
LANGUAGE plpgsql
AS $$
DECLARE
    cutoff     TIMESTAMPTZ;
    batch_size INT := 5000;
    deleted    BIGINT := 0;
    round_cnt  BIGINT;
BEGIN
    IF retention_days < 1 THEN
        RAISE EXCEPTION 'retention_days must be >= 1 (got %)', retention_days;
    END IF;
    cutoff := NOW() - (retention_days || ' days')::INTERVAL;

    LOOP
        WITH doomed AS (
            SELECT id FROM audit_log
             WHERE created_at < cutoff
             ORDER BY id
             LIMIT batch_size
             FOR UPDATE SKIP LOCKED
        )
        DELETE FROM audit_log
         WHERE id IN (SELECT id FROM doomed);
        GET DIAGNOSTICS round_cnt = ROW_COUNT;
        deleted := deleted + round_cnt;
        EXIT WHEN round_cnt < batch_size;
    END LOOP;

    RETURN deleted;
END;
$$;

COMMENT ON FUNCTION audit_log_purge(INT) IS
  'Delete audit_log rows older than N days. Batched (5000/round). Returns total deleted.';

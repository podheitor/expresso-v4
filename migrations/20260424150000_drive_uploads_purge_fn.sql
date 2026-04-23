-- Clean up stale/abandoned resumable uploads.
-- Rows have expires_at default NOW()+30d; clients that never complete upload
-- leave storage_key orphans. Function deletes expired rows and returns count.
-- Note: does NOT delete backing object storage — orphan reaper in drive service
-- sweeps those separately (storage_key tracked; this table row removal is safe).

CREATE OR REPLACE FUNCTION drive_uploads_purge_expired()
RETURNS BIGINT
LANGUAGE plpgsql
AS $$
DECLARE
    total BIGINT := 0;
    batch BIGINT;
BEGIN
    LOOP
        DELETE FROM drive_uploads
         WHERE ctid IN (
             SELECT ctid FROM drive_uploads
              WHERE expires_at < NOW()
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

COMMENT ON FUNCTION drive_uploads_purge_expired() IS
  'Delete drive_uploads rows past expires_at (batched 5000).';

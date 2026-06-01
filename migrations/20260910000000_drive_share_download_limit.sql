-- Drive shared-link download limit (optional).
--
-- A share may cap how many times it can be downloaded. `max_downloads` NULL =
-- unlimited (default, unchanged behaviour); a positive integer caps total
-- successful downloads. `download_count` tracks consumption and is incremented
-- atomically at download time so concurrent requests can't exceed the cap.
--
-- The resolver fn surfaces both so the public endpoint can enforce the limit
-- without a tenant context; the atomic increment lives in a second fn that
-- consumes one download iff the cap allows.

BEGIN;

ALTER TABLE drive_shares
    ADD COLUMN IF NOT EXISTS max_downloads  INTEGER,
    ADD COLUMN IF NOT EXISTS download_count INTEGER NOT NULL DEFAULT 0;

-- Resolver gains the two counter columns.
DROP FUNCTION IF EXISTS drive_share_resolve(TEXT);
CREATE FUNCTION drive_share_resolve(p_token_hash TEXT)
RETURNS TABLE (
    id             UUID,
    tenant_id      UUID,
    file_id        UUID,
    expires_at     TIMESTAMPTZ,
    revoked_at     TIMESTAMPTZ,
    password_hash  TEXT,
    password_salt  TEXT,
    max_downloads  INTEGER,
    download_count INTEGER
)
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    SELECT id, tenant_id, file_id, expires_at, revoked_at,
           password_hash, password_salt, max_downloads, download_count
      FROM drive_shares
     WHERE token_hash = p_token_hash
$$;

-- Atomically consume one download for a share. Increments download_count iff the
-- share is live (not revoked/expired) and under its cap (or uncapped). Returns
-- the new download_count when consumed, NULL when the request must be rejected
-- (exhausted/revoked/expired/missing). Runs as DEFINER since the public endpoint
-- has no tenant context.
CREATE OR REPLACE FUNCTION drive_share_consume(p_id UUID)
RETURNS INTEGER
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    UPDATE drive_shares
       SET download_count = download_count + 1
     WHERE id = p_id
       AND revoked_at IS NULL
       AND expires_at > now()
       AND (max_downloads IS NULL OR download_count < max_downloads)
    RETURNING download_count
$$;

COMMIT;

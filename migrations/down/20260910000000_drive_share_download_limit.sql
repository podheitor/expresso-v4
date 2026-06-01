-- DOWN: drive share download limit. Drop the consume fn + columns and restore
-- the 7-column resolver fn from the password migration.

BEGIN;

DROP FUNCTION IF EXISTS drive_share_consume(UUID);

DROP FUNCTION IF EXISTS drive_share_resolve(TEXT);
CREATE FUNCTION drive_share_resolve(p_token_hash TEXT)
RETURNS TABLE (
    id            UUID,
    tenant_id     UUID,
    file_id       UUID,
    expires_at    TIMESTAMPTZ,
    revoked_at    TIMESTAMPTZ,
    password_hash TEXT,
    password_salt TEXT
)
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    SELECT id, tenant_id, file_id, expires_at, revoked_at, password_hash, password_salt
      FROM drive_shares
     WHERE token_hash = p_token_hash
$$;

ALTER TABLE drive_shares
    DROP COLUMN IF EXISTS max_downloads,
    DROP COLUMN IF EXISTS download_count;

COMMIT;

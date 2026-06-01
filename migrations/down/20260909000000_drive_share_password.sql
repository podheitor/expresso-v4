-- DOWN: drive share password. Drop the columns and restore the 5-column
-- resolver fn from the original drive_shares migration.

BEGIN;

DROP FUNCTION IF EXISTS drive_share_resolve(TEXT);
CREATE FUNCTION drive_share_resolve(p_token_hash TEXT)
RETURNS TABLE (
    id         UUID,
    tenant_id  UUID,
    file_id    UUID,
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
)
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    SELECT id, tenant_id, file_id, expires_at, revoked_at
      FROM drive_shares
     WHERE token_hash = p_token_hash
$$;

ALTER TABLE drive_shares
    DROP COLUMN IF EXISTS password_hash,
    DROP COLUMN IF EXISTS password_salt;

COMMIT;

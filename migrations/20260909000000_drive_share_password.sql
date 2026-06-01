-- Drive shared-link password protection (optional).
--
-- A share may carry an optional password: the public download endpoint then
-- requires the correct password alongside the (already-secret) token. We store
-- a salted SHA-256 of the password — consistent with the existing token_hash
-- scheme and adding no new crypto dependency. The link token is the primary
-- secret; the password is a second factor for casual link-forwarding.
--
-- Both columns are NULL for password-less shares (the default, unchanged
-- behaviour). The resolver fn must surface them so the public endpoint can
-- verify without a tenant context.

BEGIN;

ALTER TABLE drive_shares
    ADD COLUMN IF NOT EXISTS password_hash TEXT,
    ADD COLUMN IF NOT EXISTS password_salt TEXT;

-- Resolver now returns the password material too. Replace the function in place
-- (same signature shape, two extra output columns).
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

COMMIT;

-- Personal access tokens (PATs) — phase 1: issuance + management only.
--
-- A user mints a token for CI / headless / API clients that can't run the OIDC
-- browser flow. The cleartext token is shown ONCE at creation; only its
-- sha256 is stored (revoke-by-id without recovering the secret, mirroring
-- drive_shares.token_hash). This migration adds the store + CRUD; ACCEPTING a
-- PAT as a request credential in the auth path is a LATER phase (the shared
-- auth-client validator is stateless JWT-only today). Tenant scoping via
-- explicit columns + WHERE (auth service convention, no RLS).

CREATE TABLE IF NOT EXISTS api_tokens (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID        NOT NULL,
    user_id      UUID        NOT NULL,
    name         TEXT        NOT NULL,             -- human label ("ci-deploy")
    token_hash   TEXT        NOT NULL UNIQUE,      -- sha256(cleartext)
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,                      -- set when the PAT is honored (phase 2)
    expires_at   TIMESTAMPTZ,                      -- NULL = no expiry
    revoked_at   TIMESTAMPTZ
);

-- List a user's tokens, newest first.
CREATE INDEX IF NOT EXISTS api_tokens_owner_idx
    ON api_tokens (tenant_id, user_id, created_at DESC);

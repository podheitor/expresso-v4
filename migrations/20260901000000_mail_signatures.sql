-- Migration: per-user email signatures (HTML/plain).
-- Outlook parity: a user keeps multiple named signatures, at most one marked
-- default. The partial unique index enforces the "≤ 1 default per user"
-- invariant at the DB level; the API also demotes the prior default in the
-- same write transaction so the two never disagree.

BEGIN;

CREATE TABLE IF NOT EXISTS user_signatures (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    user_id     UUID        NOT NULL REFERENCES users (id)   ON DELETE CASCADE,
    name        TEXT        NOT NULL,
    content     TEXT        NOT NULL DEFAULT '',
    -- 'html' or 'plain'; app maps unknown values to plain (never trusted HTML).
    format      TEXT        NOT NULL DEFAULT 'html',
    is_default  BOOLEAN     NOT NULL DEFAULT false,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS user_signatures_user_idx
    ON user_signatures (tenant_id, user_id);

-- At most one default signature per user.
CREATE UNIQUE INDEX IF NOT EXISTS user_signatures_one_default_idx
    ON user_signatures (tenant_id, user_id)
    WHERE is_default;

COMMIT;

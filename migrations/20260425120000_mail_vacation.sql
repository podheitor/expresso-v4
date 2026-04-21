-- Migration: Sieve vacation (out-of-office) per-user config.
-- Stores the rendered Sieve script + structured fields so UI can
-- round-trip. Ingest pipeline reads `sieve_script` during local
-- delivery to execute RFC 5230 vacation action.

BEGIN;

CREATE TABLE IF NOT EXISTS user_vacation (
    user_id       UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    tenant_id     UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    enabled       BOOLEAN NOT NULL DEFAULT false,
    starts_at     TIMESTAMPTZ,
    ends_at       TIMESTAMPTZ,
    subject       TEXT NOT NULL DEFAULT 'Out of office',
    body          TEXT NOT NULL DEFAULT '',
    -- RFC 5230 :days parameter — min interval between auto-replies
    -- to the same sender (1-365).
    interval_days INT  NOT NULL DEFAULT 7
                  CHECK (interval_days BETWEEN 1 AND 365),
    sieve_script  TEXT NOT NULL DEFAULT '',
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_user_vacation_tenant ON user_vacation (tenant_id);

COMMIT;

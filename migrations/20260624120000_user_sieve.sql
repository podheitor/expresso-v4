-- Migration: Per-user Sieve filter scripts (inbox rules).
-- Separate from user_vacation (which holds only the rendered vacation Sieve).
-- A user may have ONE active filter script; UI compiles rules → Sieve text.
-- Ingest runs vacation THEN filter script in sequence.

BEGIN;

CREATE TABLE IF NOT EXISTS user_sieve (
    user_id     UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    tenant_id   UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    enabled     BOOLEAN NOT NULL DEFAULT true,
    script      TEXT NOT NULL DEFAULT '',
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_user_sieve_tenant ON user_sieve (tenant_id);

COMMIT;

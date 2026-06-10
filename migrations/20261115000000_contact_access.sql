-- Migration: per-user recent-contact tracking (People "recent" list).
-- One row per (user, contact); each view upserts `accessed_at = now()`, so the
-- table stays small (one row per contact ever viewed) and "recents" is just an
-- ORDER BY accessed_at DESC. Deleting the contact cascades.

BEGIN;

CREATE TABLE IF NOT EXISTS contact_access (
    tenant_id    UUID        NOT NULL,
    user_id      UUID        NOT NULL,
    contact_id   UUID        NOT NULL REFERENCES contacts (id) ON DELETE CASCADE,
    accessed_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id, contact_id)
);

CREATE INDEX IF NOT EXISTS contact_access_recent_idx
    ON contact_access (tenant_id, user_id, accessed_at DESC);

COMMIT;

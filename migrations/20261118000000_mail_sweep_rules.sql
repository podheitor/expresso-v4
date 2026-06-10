-- Migration: per-user Sweep rules (Outlook "Sweep").
-- A rule moves messages from a given sender that are older than N days into a
-- target folder (typically Trash or Archive). A background worker applies
-- enabled rules periodically. Sender stored lowercased.

BEGIN;

CREATE TABLE IF NOT EXISTS mail_sweep_rules (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID        NOT NULL,
    user_id         UUID        NOT NULL,
    sender_address  TEXT        NOT NULL,
    older_than_days INTEGER     NOT NULL DEFAULT 7,
    target_folder   TEXT        NOT NULL DEFAULT 'Trash',
    enabled         BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS mail_sweep_rules_user_idx
    ON mail_sweep_rules (tenant_id, user_id);

COMMIT;

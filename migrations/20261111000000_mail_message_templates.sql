-- Migration: per-user email message templates (canned responses).
-- A user keeps named templates with an optional subject and a body; the
-- compose screen offers them in a dropdown. Mirrors mail_flag_presets /
-- user_signatures (per-(tenant,user) rows, indexed for the list query).
-- RLS-friendly: tenant_id is an explicit column filtered by every query.

BEGIN;

CREATE TABLE IF NOT EXISTS mail_message_templates (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL,
    user_id     UUID        NOT NULL,
    name        TEXT        NOT NULL,
    subject     TEXT        NOT NULL DEFAULT '',
    body        TEXT        NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS mail_message_templates_user_idx
    ON mail_message_templates (tenant_id, user_id);

COMMIT;

-- Migration: per-user safe senders (Outlook "safe senders" allow-list).
-- Inbound mail whose From matches a safe address is always delivered to the
-- Inbox — it overrides both the blocked list and Sieve spam-filing. Addresses
-- stored lowercased; (tenant,user,address) is unique.

BEGIN;

CREATE TABLE IF NOT EXISTS mail_safe_senders (
    tenant_id   UUID        NOT NULL,
    user_id     UUID        NOT NULL,
    address     TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id, address)
);

CREATE INDEX IF NOT EXISTS mail_safe_senders_user_idx
    ON mail_safe_senders (tenant_id, user_id);

COMMIT;

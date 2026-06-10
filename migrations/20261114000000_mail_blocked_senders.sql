-- Migration: per-user blocked senders (Outlook "blocked senders" list).
-- Inbound mail whose From matches a blocked address is routed to Spam at
-- delivery time (after Sieve, before the message lands in the inbox).
-- Addresses are stored lowercased; (tenant,user,address) is unique.

BEGIN;

CREATE TABLE IF NOT EXISTS mail_blocked_senders (
    tenant_id   UUID        NOT NULL,
    user_id     UUID        NOT NULL,
    address     TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id, address)
);

CREATE INDEX IF NOT EXISTS mail_blocked_senders_user_idx
    ON mail_blocked_senders (tenant_id, user_id);

COMMIT;

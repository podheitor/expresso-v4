-- Migration: per-user contact favorites (starred contacts).
-- Favorites are a personal overlay, NOT part of the shared vCard — a user
-- stars contacts in any addressbook they can see, without mutating the card.
-- (tenant_id, user_id, contact_id) is unique; deleting the contact cascades.

BEGIN;

CREATE TABLE IF NOT EXISTS contact_favorites (
    tenant_id   UUID        NOT NULL,
    user_id     UUID        NOT NULL,
    contact_id  UUID        NOT NULL REFERENCES contacts (id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id, contact_id)
);

CREATE INDEX IF NOT EXISTS contact_favorites_user_idx
    ON contact_favorites (tenant_id, user_id);

COMMIT;

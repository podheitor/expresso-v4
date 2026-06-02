-- Contact activity log (audit trail) — mirrors drive_file_activity.
--
-- Append-only record of contact mutations (create/update/delete) and any
-- explicitly recorded events. Lets the contacts service offer the same audit
-- trail Drive/Calendar/Chat/Mail already have. Tenant-scoped via explicit
-- columns + WHERE (sibling contacts child-table convention).

BEGIN;

CREATE TABLE IF NOT EXISTS contact_activity (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    contact_id  UUID NOT NULL,
    tenant_id   UUID NOT NULL,
    user_id     UUID NOT NULL,
    action      TEXT NOT NULL,   -- e.g. create, update, delete
    detail      JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS contact_activity_contact_idx
    ON contact_activity (tenant_id, contact_id, created_at DESC);

CREATE INDEX IF NOT EXISTS contact_activity_user_idx
    ON contact_activity (tenant_id, user_id, created_at DESC);

COMMIT;

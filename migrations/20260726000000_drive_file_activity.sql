-- Sprint #410: drive file activity log (audit trail)
CREATE TABLE IF NOT EXISTS drive_file_activity (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    file_id     UUID NOT NULL,
    tenant_id   UUID NOT NULL,
    user_id     UUID NOT NULL,
    action      TEXT NOT NULL,   -- e.g. upload, download, rename, share, delete, comment, lock, unlock, star, unstar
    detail      JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS drive_file_activity_file_idx
    ON drive_file_activity (tenant_id, file_id, created_at DESC);

CREATE INDEX IF NOT EXISTS drive_file_activity_user_idx
    ON drive_file_activity (tenant_id, user_id, created_at DESC);

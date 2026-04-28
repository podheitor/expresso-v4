ALTER TABLE drive_files ADD COLUMN IF NOT EXISTS starred_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS drive_files_starred_idx
    ON drive_files (tenant_id, owner_user_id, starred_at DESC)
    WHERE starred_at IS NOT NULL AND deleted_at IS NULL;

ALTER TABLE drive_files ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS drive_files_expires_at_idx
    ON drive_files (expires_at)
    WHERE expires_at IS NOT NULL AND deleted_at IS NULL;

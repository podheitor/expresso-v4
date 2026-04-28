ALTER TABLE drive_files ADD COLUMN IF NOT EXISTS locked_by  UUID;
ALTER TABLE drive_files ADD COLUMN IF NOT EXISTS locked_at  TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS drive_files_locked_idx
    ON drive_files (locked_by)
    WHERE locked_by IS NOT NULL AND deleted_at IS NULL;

ALTER TABLE meetings
    ADD COLUMN IF NOT EXISTS recording_started_at TIMESTAMPTZ;

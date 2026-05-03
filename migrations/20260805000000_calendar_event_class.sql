-- Adds RFC 5545 §3.8.1.3 CLASS property column to calendar_events.
-- Same shape as `status` (TEXT + CHECK enum + nullable). Default NULL
-- maps to RFC default "PUBLIC" without forcing a backfill on existing rows.

ALTER TABLE calendar_events
    ADD COLUMN IF NOT EXISTS class TEXT
        CHECK (class IN ('PUBLIC','PRIVATE','CONFIDENTIAL') OR class IS NULL);

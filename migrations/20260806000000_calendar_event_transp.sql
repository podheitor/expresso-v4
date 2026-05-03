-- Adds RFC 5545 §3.8.2.7 TRANSP property column to calendar_events.
-- Same shape as `class` (#555) and `status` (TEXT + CHECK enum + nullable).
-- Default NULL maps to RFC default "OPAQUE" without forcing a backfill.

ALTER TABLE calendar_events
    ADD COLUMN IF NOT EXISTS transp TEXT
        CHECK (transp IN ('OPAQUE','TRANSPARENT') OR transp IS NULL);

-- Sprint #407: alarm delivery tracking
ALTER TABLE calendar_event_alarms
    ADD COLUMN IF NOT EXISTS delivered_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS calendar_event_alarms_due_idx
    ON calendar_event_alarms (trigger_abs)
    WHERE trigger_abs IS NOT NULL AND delivered_at IS NULL;

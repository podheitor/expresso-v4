-- Per-notification snooze (temporal muting).
--
-- A user may snooze a notification: it's suppressed from unread digests until
-- `snooze_until`, then reappears. NULL (the default) = not snoozed. Only the
-- digest/badge path honors it; mark-all-read still clears snoozed rows.

ALTER TABLE notifications
    ADD COLUMN IF NOT EXISTS snooze_until TIMESTAMPTZ;

-- Digest filters out rows still snoozed; index the active-snooze predicate.
CREATE INDEX IF NOT EXISTS notifications_snooze_idx
    ON notifications (tenant_id, user_id, snooze_until)
    WHERE snooze_until IS NOT NULL;

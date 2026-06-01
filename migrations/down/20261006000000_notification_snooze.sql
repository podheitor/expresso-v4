-- Revert: drop the per-notification snooze column.

DROP INDEX IF EXISTS notifications_snooze_idx;
ALTER TABLE notifications DROP COLUMN IF EXISTS snooze_until;

-- Recurring meetings: store the RFC 5545 RRULE string.
--
-- `is_recurring` (existing bool) flags a meeting as a series; `rrule` carries
-- the actual recurrence rule used to expand `scheduled_for`/`ends_at` into
-- concrete instances (shared expresso-rrule expander, same as calendar).
-- A series with is_recurring = TRUE but rrule = NULL degrades to a single
-- occurrence at scheduled_for (no rule to expand).

BEGIN;

ALTER TABLE meetings ADD COLUMN rrule TEXT;

COMMIT;

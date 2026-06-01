-- Recurring tasks: add RRULE to calendar_tasks (RFC 5545 VTODO §3.8.5.3).
--
-- A task may recur (daily standup, weekly report). `rrule` stores the raw
-- recurrence rule string (e.g. "FREQ=WEEKLY;BYDAY=MO"); NULL = a one-off task
-- (the default, unchanged). Expansion is virtual at read time via the shared
-- expresso-rrule library, mirroring events — no per-instance rows are stored.

ALTER TABLE calendar_tasks
    ADD COLUMN IF NOT EXISTS rrule TEXT;

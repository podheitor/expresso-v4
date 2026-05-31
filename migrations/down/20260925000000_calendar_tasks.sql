-- Revert: drop the calendar tasks (VTODO) table.

DROP TRIGGER IF EXISTS calendar_tasks_touch_trg ON calendar_tasks;
DROP FUNCTION IF EXISTS calendar_tasks_touch();
DROP TABLE IF EXISTS calendar_tasks;

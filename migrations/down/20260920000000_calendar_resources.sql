-- Revert: drop the calendar resource registry + per-event booking index.

DROP TRIGGER IF EXISTS calendar_resources_touch_trg ON calendar_resources;
DROP FUNCTION IF EXISTS calendar_resources_touch();
DROP TABLE IF EXISTS calendar_event_resources;
DROP TABLE IF EXISTS calendar_resources;

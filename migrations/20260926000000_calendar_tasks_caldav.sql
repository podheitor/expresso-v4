-- CalDAV support for calendar tasks (VTODO).
--
-- A task created over CalDAV (PUT of a VCALENDAR/VTODO body) keeps the raw
-- iCalendar verbatim for roundtrip fidelity, exactly like calendar_events does
-- with ical_raw. `etag` is the SHA-256 of that body, used for If-Match / sync.
-- Both are NULL for tasks created via the REST API (no source iCalendar); GET
-- over CalDAV serializes those from the structured columns on the fly.

ALTER TABLE calendar_tasks
    ADD COLUMN IF NOT EXISTS ical_raw TEXT,
    ADD COLUMN IF NOT EXISTS etag     TEXT;

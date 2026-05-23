-- DOWN: DAV tombstones
BEGIN;

DROP INDEX IF EXISTS idx_contact_tombstones_tenant;
DROP INDEX IF EXISTS idx_contact_tombstones_sync;
DROP TABLE IF EXISTS contact_tombstones;

DROP INDEX IF EXISTS idx_event_tombstones_tenant;
DROP INDEX IF EXISTS idx_event_tombstones_sync;
DROP TABLE IF EXISTS calendar_event_tombstones;

DROP INDEX IF EXISTS idx_events_calendar_last_ctag;
DROP INDEX IF EXISTS idx_contacts_book_last_ctag;

COMMIT;

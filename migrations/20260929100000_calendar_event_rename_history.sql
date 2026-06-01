-- Calendar event rename history + undo.
--
-- A PATCH that changes an event's SUMMARY ("rename") records (old, new) so a
-- user can review past renames and undo one — mirroring drive_file_rename_history
-- and mail_folder_rename_history. The history row is written in the same tx as
-- the patch, so the trail can't drift. Tenant scoping via explicit columns +
-- WHERE (sibling calendar child-table convention, no RLS).

CREATE TABLE IF NOT EXISTS calendar_event_rename_history (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID        NOT NULL,
    calendar_id  UUID        NOT NULL,
    event_id     UUID        NOT NULL REFERENCES calendar_events(id) ON DELETE CASCADE,
    old_summary  TEXT        NOT NULL,
    new_summary  TEXT        NOT NULL,
    renamed_by   UUID        NOT NULL,
    renamed_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS calendar_event_rename_history_event_idx
    ON calendar_event_rename_history (tenant_id, event_id, renamed_at DESC);

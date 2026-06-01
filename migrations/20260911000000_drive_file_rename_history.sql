-- Drive file rename history + undo.
--
-- Every successful file/folder rename records (old_name, new_name) so a user can
-- review past renames and undo one. Mirrors mail_folder_rename_history. The
-- history row is written in the same transaction as the rename, so the audit
-- trail can't drift from the live name. Tenant isolation via RLS.

BEGIN;

CREATE TABLE IF NOT EXISTS drive_file_rename_history (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants(id)     ON DELETE CASCADE,
    file_id     UUID        NOT NULL REFERENCES drive_files(id) ON DELETE CASCADE,
    old_name    TEXT        NOT NULL,
    new_name    TEXT        NOT NULL,
    renamed_by  UUID        NOT NULL REFERENCES users(id)       ON DELETE CASCADE,
    renamed_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS drive_file_rename_history_file_idx
    ON drive_file_rename_history (tenant_id, file_id, renamed_at DESC);

ALTER TABLE drive_file_rename_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE drive_file_rename_history FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_drive_file_rename_history ON drive_file_rename_history;
CREATE POLICY rls_drive_file_rename_history ON drive_file_rename_history
    USING      (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

COMMIT;

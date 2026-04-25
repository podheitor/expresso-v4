-- WOPI lock state (MS-WOPI Lock/Unlock/RefreshLock/UnlockAndRelock).
--
-- One row per locked file. Caller-supplied lock_token is opaque text up
-- to 1024 chars (WOPI spec). expires_at enforces the 30-minute lock TTL
-- prescribed by the spec — clients are expected to RefreshLock before
-- it elapses. Released locks delete the row.

BEGIN;

CREATE TABLE IF NOT EXISTS drive_wopi_locks (
    file_id     UUID        PRIMARY KEY REFERENCES drive_files(id) ON DELETE CASCADE,
    tenant_id   UUID        NOT NULL    REFERENCES tenants(id)     ON DELETE CASCADE,
    lock_token  TEXT        NOT NULL    CHECK (length(lock_token) BETWEEN 1 AND 1024),
    locked_by   UUID        NOT NULL    REFERENCES users(id)       ON DELETE CASCADE,
    acquired_at TIMESTAMPTZ NOT NULL    DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL    DEFAULT (now() + INTERVAL '30 minutes')
);

CREATE INDEX IF NOT EXISTS ix_drive_wopi_locks_tenant  ON drive_wopi_locks (tenant_id);
CREATE INDEX IF NOT EXISTS ix_drive_wopi_locks_expires ON drive_wopi_locks (expires_at);

ALTER TABLE drive_wopi_locks ENABLE ROW LEVEL SECURITY;
ALTER TABLE drive_wopi_locks FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_drive_wopi_locks ON drive_wopi_locks;
CREATE POLICY rls_drive_wopi_locks ON drive_wopi_locks
    USING      (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

COMMIT;

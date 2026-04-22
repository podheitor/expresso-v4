-- Drive resumable uploads (tus.io protocol v1.0.0 server state).
--
-- Cada upload em progresso → 1 linha. storage_key aponta blob temporário em
-- data_root/<key>.part. Ao completar (offset == total_size), upload é
-- promovido para drive_files (com quota check + versionamento) e a linha
-- aqui é removida.
--
-- expires_at garante GC automático de uploads abandonados (30 dias default).

BEGIN;

CREATE TABLE IF NOT EXISTS drive_uploads (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(id)         ON DELETE CASCADE,
    owner_user_id   UUID        NOT NULL REFERENCES users(id)            ON DELETE CASCADE,
    parent_id       UUID                 REFERENCES drive_files(id)      ON DELETE SET NULL,
    name            TEXT        NOT NULL,
    mime_type       TEXT,
    total_size      BIGINT      NOT NULL CHECK (total_size >= 0),
    offset_bytes    BIGINT      NOT NULL DEFAULT 0 CHECK (offset_bytes >= 0),
    storage_key     TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at      TIMESTAMPTZ NOT NULL DEFAULT (now() + INTERVAL '30 days'),
    CHECK (offset_bytes <= total_size)
);

CREATE INDEX IF NOT EXISTS ix_drive_uploads_tenant        ON drive_uploads (tenant_id);
CREATE INDEX IF NOT EXISTS ix_drive_uploads_expires       ON drive_uploads (expires_at);

ALTER TABLE drive_uploads ENABLE ROW LEVEL SECURITY;
ALTER TABLE drive_uploads FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_drive_uploads ON drive_uploads;
CREATE POLICY rls_drive_uploads ON drive_uploads
    USING      (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

COMMIT;

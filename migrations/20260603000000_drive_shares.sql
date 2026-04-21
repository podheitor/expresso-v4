-- Drive shared links (Phase 3).
--
-- Link de compartilhamento público por token. Banco armazena sha256(token)
-- p/ permitir revogação por id sem reversibilidade ao dono do link. TTL
-- e escopo (read-only) enforçados no endpoint público /drive/share/:token.

BEGIN;

CREATE TABLE IF NOT EXISTS drive_shares (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants(id)     ON DELETE CASCADE,
    file_id     UUID        NOT NULL REFERENCES drive_files(id) ON DELETE CASCADE,
    token_hash  TEXT        NOT NULL UNIQUE,
    permission  TEXT        NOT NULL DEFAULT 'read' CHECK (permission IN ('read')),
    created_by  UUID        NOT NULL REFERENCES users(id)       ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL,
    revoked_at  TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_drive_shares_tenant ON drive_shares (tenant_id);
CREATE INDEX IF NOT EXISTS idx_drive_shares_file   ON drive_shares (file_id);

ALTER TABLE drive_shares ENABLE ROW LEVEL SECURITY;
ALTER TABLE drive_shares FORCE  ROW LEVEL SECURITY;

-- RLS ativa p/ API autenticada; endpoint público usa conexão privilegiada
-- via função SQL separada (evita bypass de RLS pelo serviço).
DROP POLICY IF EXISTS rls_drive_shares ON drive_shares;
CREATE POLICY rls_drive_shares ON drive_shares
    USING      (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

-- Função SECURITY DEFINER p/ resolver um token sem contexto de tenant
-- (endpoint público). Retorna file_id + tenant_id + expires_at + revoked_at.
CREATE OR REPLACE FUNCTION drive_share_resolve(p_token_hash TEXT)
RETURNS TABLE (
    id         UUID,
    tenant_id  UUID,
    file_id    UUID,
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
)
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    SELECT id, tenant_id, file_id, expires_at, revoked_at
      FROM drive_shares
     WHERE token_hash = p_token_hash
$$;

COMMIT;

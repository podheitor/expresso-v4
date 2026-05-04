-- Sprint #583: calendar_event_history — log de mudanças de etag/sequence por evento.
--
-- Captura cada vez que um evento é modificado via PUT (full replace) ou PATCH
-- (partial update). Cada row registra o novo etag, o sequence resultante, e a
-- operação que causou a mudança. Não usa trigger — inserido explicitamente pelos
-- métodos EventRepo::update e EventRepo::patch_fields após o UPDATE.
--
-- Retenção: sem política automática de expiry nessa migração; futuras migrações
-- podem adicionar um índice por changed_at e uma job de cleanup.

CREATE TABLE IF NOT EXISTS calendar_event_history (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID        NOT NULL,
    calendar_id  UUID        NOT NULL,
    event_id     UUID        NOT NULL,
    etag         TEXT        NOT NULL,
    sequence     INTEGER     NOT NULL,
    op           TEXT        NOT NULL CHECK (op IN ('PUT', 'PATCH')),
    changed_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_cal_event_history_event
    ON calendar_event_history (tenant_id, event_id, changed_at DESC);

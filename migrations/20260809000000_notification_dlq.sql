-- Sprint #572: webhook retry DLQ para notifications.
-- Armazena notificações cujo webhook falhou após MAX_WEBHOOK_ATTEMPTS tentativas.
-- `payload` preserva o JSON completo da notificação para re-envio manual ou automático.

CREATE TABLE IF NOT EXISTS notification_dlq (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID,
    user_id     UUID,
    kind        TEXT        NOT NULL,
    payload     JSONB       NOT NULL,
    attempts    INT         NOT NULL DEFAULT 3,
    last_error  TEXT,
    failed_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS notification_dlq_failed_at ON notification_dlq (failed_at DESC);

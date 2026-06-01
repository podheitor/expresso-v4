-- Flow webhook delivery log.
--
-- A flow rule with a `webhook` action POSTs to an external URL when a message
-- matches (executed in expresso-mail's ingest path). This table records each
-- attempt's outcome so a user can debug failing automations — previously the
-- POST was fire-and-forget with the result discarded. One row per delivery
-- attempt. Tenant scoping via explicit columns + WHERE (mail-service convention).

CREATE TABLE IF NOT EXISTS flow_webhook_log (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL,
    user_id     UUID        NOT NULL,
    message_id  UUID,
    url         TEXT        NOT NULL,
    status_code INTEGER,                            -- HTTP status, NULL on transport error
    ok          BOOLEAN     NOT NULL DEFAULT FALSE, -- 2xx delivered
    error       TEXT,                               -- transport/timeout error, NULL on HTTP response
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Recent deliveries for a user, newest first (debug dashboard).
CREATE INDEX IF NOT EXISTS flow_webhook_log_user_idx
    ON flow_webhook_log (tenant_id, user_id, created_at DESC);

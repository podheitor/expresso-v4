-- Sprint #403: WebPush VAPID subscription storage
CREATE TABLE IF NOT EXISTS notification_push_subscriptions (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID NOT NULL,
    user_id     UUID NOT NULL,
    endpoint    TEXT NOT NULL,
    p256dh      TEXT NOT NULL,   -- client public key (base64url)
    auth        TEXT NOT NULL,   -- auth secret (base64url)
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, user_id, endpoint)
);

CREATE INDEX IF NOT EXISTS notification_push_subscriptions_user_idx
    ON notification_push_subscriptions (tenant_id, user_id);

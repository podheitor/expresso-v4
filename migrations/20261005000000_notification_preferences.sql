-- Per-user notification preferences (mute by kind).
--
-- A user may disable specific notification kinds (e.g. "flags_changed"). A row
-- exists only for kinds the user has explicitly set; the ABSENCE of a row means
-- enabled (notifications default-on). The ingest path (`internal_notify`)
-- consults this table and drops a notification for a disabled kind before any
-- fan-out (SSE / Redis / webhook / persistence). Tenant scoping via explicit
-- columns + WHERE, matching the notifications table (no RLS in this service).

CREATE TABLE IF NOT EXISTS notification_preferences (
    tenant_id   UUID        NOT NULL,
    user_id     UUID        NOT NULL,
    kind        TEXT        NOT NULL,
    enabled     BOOLEAN     NOT NULL DEFAULT true,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id, kind)
);

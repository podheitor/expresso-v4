-- Per-user working (business) hours for the calendar.
--
-- One row per (user, weekday) defines an availability window. Clients use this
-- to render business hours and the scheduler can later prefer in-hours slots
-- (free/busy integration is a follow-up). weekday: 0=Sunday .. 6=Saturday.
-- start_minute/end_minute are minutes-from-midnight in the user's timezone
-- (0..1440), start < end. Absence of a row for a weekday = not working that day.
-- Tenant-scoped via explicit columns + WHERE.

BEGIN;

CREATE TABLE IF NOT EXISTS user_working_hours (
    tenant_id    UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id      UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    weekday      SMALLINT    NOT NULL CHECK (weekday BETWEEN 0 AND 6),
    start_minute INTEGER     NOT NULL CHECK (start_minute BETWEEN 0 AND 1440),
    end_minute   INTEGER     NOT NULL CHECK (end_minute BETWEEN 0 AND 1440 AND end_minute > start_minute),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id, weekday)
);

CREATE INDEX IF NOT EXISTS user_working_hours_user_idx
    ON user_working_hours (tenant_id, user_id);

COMMIT;

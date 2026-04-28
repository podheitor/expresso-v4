CREATE TABLE IF NOT EXISTS meeting_breakout_rooms (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id  UUID        NOT NULL,
    tenant_id   UUID        NOT NULL,
    name        TEXT        NOT NULL,
    created_by  UUID        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS meeting_breakout_rooms_meeting_idx
    ON meeting_breakout_rooms (meeting_id, tenant_id);

CREATE TABLE IF NOT EXISTS meeting_breakout_participants (
    room_id     UUID        NOT NULL,
    meeting_id  UUID        NOT NULL,
    tenant_id   UUID        NOT NULL,
    user_id     UUID        NOT NULL,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (room_id, user_id)
);

CREATE INDEX IF NOT EXISTS meeting_breakout_participants_room_idx
    ON meeting_breakout_participants (room_id, tenant_id);

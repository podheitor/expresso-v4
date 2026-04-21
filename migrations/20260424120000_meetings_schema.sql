-- Meetings metadata — Jitsi room registry + participant ACL.
--
-- Jitsi rooms are ephemeral (auto-created on first join). This table is the
-- Expresso-side source of truth for: title/scheduling/ACL, tenant scoping,
-- linkage to chat_channels, and audit (who created what, when, how long).

BEGIN;

CREATE TABLE meetings (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL,
    room_name       TEXT NOT NULL UNIQUE,
    title           TEXT NOT NULL,
    channel_id      UUID,
    created_by      UUID NOT NULL,
    scheduled_for   TIMESTAMPTZ,
    ends_at         TIMESTAMPTZ,
    is_recurring    BOOLEAN NOT NULL DEFAULT FALSE,
    is_archived     BOOLEAN NOT NULL DEFAULT FALSE,
    lobby_enabled   BOOLEAN NOT NULL DEFAULT TRUE,
    password        TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX meetings_tenant_idx  ON meetings (tenant_id);
CREATE INDEX meetings_channel_idx ON meetings (channel_id) WHERE channel_id IS NOT NULL;
CREATE INDEX meetings_sched_idx   ON meetings (tenant_id, scheduled_for) WHERE scheduled_for IS NOT NULL;

CREATE TABLE meeting_participants (
    meeting_id  UUID NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    tenant_id   UUID NOT NULL,
    user_id     UUID NOT NULL,
    role        TEXT NOT NULL DEFAULT 'participant'
                CHECK (role IN ('moderator','participant')),
    invited_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (meeting_id, user_id)
);

CREATE INDEX meeting_participants_tenant_user_idx
    ON meeting_participants (tenant_id, user_id);

-- updated_at trigger
CREATE OR REPLACE FUNCTION meetings_touch() RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER meetings_touch_trg
    BEFORE UPDATE ON meetings
    FOR EACH ROW EXECUTE FUNCTION meetings_touch();

-- RLS: bootstrap-bypass convention (same as chat/calendar/contacts).
ALTER TABLE meetings             ENABLE ROW LEVEL SECURITY;
ALTER TABLE meeting_participants ENABLE ROW LEVEL SECURITY;

CREATE POLICY meetings_isolation ON meetings
    USING (current_setting('app.tenant_id', true) IS NULL
           OR tenant_id = current_setting('app.tenant_id', true)::uuid);

CREATE POLICY meeting_participants_isolation ON meeting_participants
    USING (current_setting('app.tenant_id', true) IS NULL
           OR tenant_id = current_setting('app.tenant_id', true)::uuid);

COMMIT;

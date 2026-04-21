-- Chat metadata: channels (tenant-scoped business objects paired with Matrix rooms)
-- and membership (logical — authoritative membership lives in Synapse; this
-- table is the expresso-side index for fast tenant listings + access control).

BEGIN;

CREATE TABLE chat_channels (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL,
    matrix_room_id  TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    topic           TEXT,
    kind            TEXT NOT NULL DEFAULT 'team'
                    CHECK (kind IN ('team','direct','announcement','project')),
    team_id         UUID,
    created_by      UUID NOT NULL,
    is_archived     BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX chat_channels_tenant_idx  ON chat_channels (tenant_id);
CREATE INDEX chat_channels_team_idx    ON chat_channels (team_id) WHERE team_id IS NOT NULL;

CREATE TABLE chat_channel_members (
    channel_id  UUID NOT NULL REFERENCES chat_channels(id) ON DELETE CASCADE,
    tenant_id   UUID NOT NULL,
    user_id     UUID NOT NULL,
    role        TEXT NOT NULL DEFAULT 'member'
                CHECK (role IN ('owner','admin','member','guest')),
    joined_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (channel_id, user_id)
);

CREATE INDEX chat_members_tenant_user_idx ON chat_channel_members (tenant_id, user_id);

-- updated_at trigger for channels
CREATE OR REPLACE FUNCTION chat_channels_touch() RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER chat_channels_touch_trg
    BEFORE UPDATE ON chat_channels
    FOR EACH ROW EXECUTE FUNCTION chat_channels_touch();

-- RLS: allow bootstrap (app.tenant_id unset → bypass) OR match current setting.
ALTER TABLE chat_channels        ENABLE ROW LEVEL SECURITY;
ALTER TABLE chat_channel_members ENABLE ROW LEVEL SECURITY;

CREATE POLICY chat_channels_isolation ON chat_channels
    USING (current_setting('app.tenant_id', true) IS NULL
           OR tenant_id = current_setting('app.tenant_id', true)::uuid);

CREATE POLICY chat_members_isolation ON chat_channel_members
    USING (current_setting('app.tenant_id', true) IS NULL
           OR tenant_id = current_setting('app.tenant_id', true)::uuid);

COMMIT;

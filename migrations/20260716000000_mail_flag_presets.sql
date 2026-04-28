CREATE TABLE IF NOT EXISTS mail_flag_presets (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL,
    user_id     UUID        NOT NULL,
    name        TEXT        NOT NULL,
    flags       JSONB       NOT NULL DEFAULT '[]',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS mail_flag_presets_user_idx
    ON mail_flag_presets (tenant_id, user_id);

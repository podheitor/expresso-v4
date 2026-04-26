-- Workflow rules for expresso-flows
-- Each rule belongs to one user+tenant; evaluated in ascending priority order.
CREATE TABLE IF NOT EXISTS flow_rules (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID        NOT NULL,
    tenant_id   UUID        NOT NULL,
    name        TEXT        NOT NULL DEFAULT '',
    enabled     BOOLEAN     NOT NULL DEFAULT TRUE,
    priority    INT         NOT NULL DEFAULT 10,
    -- JSON array of {field, op, value} objects
    conditions  JSONB       NOT NULL DEFAULT '[]',
    -- JSON array of {type, params} objects
    actions     JSONB       NOT NULL DEFAULT '[]',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS flow_rules_user_tenant
    ON flow_rules (tenant_id, user_id, enabled, priority);

-- Retention policies for expresso-compliance
-- One policy per tenant + folder combination.
CREATE TABLE IF NOT EXISTS retention_policies (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID        NOT NULL,
    -- NULL = applies to all folders; non-NULL = specific folder name
    folder_name  TEXT,
    -- Number of days after which messages are subject to this policy
    retain_days  INT         NOT NULL CHECK (retain_days > 0),
    -- "delete" = hard-delete; "archive" = move to compliance archive store (future)
    action       TEXT        NOT NULL DEFAULT 'delete' CHECK (action IN ('delete', 'archive')),
    enabled      BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- One policy per (tenant, folder) combination
    UNIQUE (tenant_id, folder_name)
);

CREATE INDEX IF NOT EXISTS retention_policies_tenant
    ON retention_policies (tenant_id, enabled);

-- Compliance archive: journal copies of messages (append-only)
CREATE TABLE IF NOT EXISTS compliance_archive (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID        NOT NULL,
    user_id      UUID        NOT NULL,
    original_id  UUID,           -- original messages.id, may be NULL if already expunged
    body_path    TEXT        NOT NULL,
    from_addr    TEXT,
    to_addrs     JSONB       NOT NULL DEFAULT '[]',
    subject      TEXT,
    archived_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    size_bytes   INT         NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS compliance_archive_tenant_user
    ON compliance_archive (tenant_id, user_id, archived_at DESC);

CREATE TABLE IF NOT EXISTS compliance_tenant_retention (
    tenant_id   UUID PRIMARY KEY,
    retain_days INTEGER NOT NULL DEFAULT 365 CHECK (retain_days > 0),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

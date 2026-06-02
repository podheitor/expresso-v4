-- Billing: per-tenant plans + internal invoices (fixed price per plan).
--
-- The tenant's plan already lives in `tenants.plan` (standard|professional|
-- enterprise). `billing_plans` attaches a monthly price to each plan; invoices
-- are generated per tenant per period at the plan's current price (fixed-price
-- model — no usage-based charging). No external payment gateway: an invoice is
-- marked paid out-of-band by an admin.

-- ─── Plans (global catalogue, not tenant-scoped) ─────────────────────────────
CREATE TABLE IF NOT EXISTS billing_plans (
    plan                TEXT PRIMARY KEY
                        CHECK (plan IN ('standard', 'professional', 'enterprise')),
    display_name        TEXT        NOT NULL,
    monthly_price_cents BIGINT      NOT NULL CHECK (monthly_price_cents >= 0),
    currency            TEXT        NOT NULL DEFAULT 'BRL' CHECK (char_length(currency) = 3),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Seed the three existing plans (prices in cents; adjust via the admin UI).
INSERT INTO billing_plans (plan, display_name, monthly_price_cents, currency) VALUES
    ('standard',     'Standard',     4900,  'BRL'),
    ('professional', 'Professional', 9900,  'BRL'),
    ('enterprise',   'Enterprise',   29900, 'BRL')
ON CONFLICT (plan) DO NOTHING;

-- ─── Invoices (per tenant, per billing period) ───────────────────────────────
CREATE TABLE IF NOT EXISTS billing_invoices (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    -- Billing period as the first day of the month it covers.
    period       DATE        NOT NULL,
    plan         TEXT        NOT NULL,
    amount_cents BIGINT      NOT NULL CHECK (amount_cents >= 0),
    currency     TEXT        NOT NULL DEFAULT 'BRL',
    status       TEXT        NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending', 'paid', 'void')),
    issued_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    paid_at      TIMESTAMPTZ,
    -- One invoice per tenant per period.
    UNIQUE (tenant_id, period)
);

CREATE INDEX IF NOT EXISTS billing_invoices_tenant_idx
    ON billing_invoices (tenant_id, period DESC);
CREATE INDEX IF NOT EXISTS billing_invoices_status_idx
    ON billing_invoices (status, period DESC);

ALTER TABLE billing_invoices ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing_invoices FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_billing_invoices ON billing_invoices;
CREATE POLICY rls_billing_invoices ON billing_invoices
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

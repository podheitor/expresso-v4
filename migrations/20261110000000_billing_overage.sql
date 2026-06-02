-- Billing usage-based add-ons: per-plan allowances + overage pricing, and
-- per-invoice line items so an invoice can carry a base charge plus overage.
--
-- Backwards-compatible by default: every new plan column defaults to an
-- allowance/price that makes overage a no-op (0 overage price → no extra
-- charge), so existing fixed-price invoices are unchanged until an admin sets
-- an overage price. `billing_invoices.amount_cents` stays the invoice TOTAL
-- (sum of its lines), so existing readers keep working.

-- ─── Plan allowances + overage prices ────────────────────────────────────────
ALTER TABLE billing_plans
    ADD COLUMN IF NOT EXISTS included_seats               INT    NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS seat_overage_cents           BIGINT NOT NULL DEFAULT 0
        CHECK (seat_overage_cents >= 0),
    ADD COLUMN IF NOT EXISTS included_storage_gb          INT    NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS storage_overage_cents_per_gb BIGINT NOT NULL DEFAULT 0
        CHECK (storage_overage_cents_per_gb >= 0);

-- ─── Invoice line items ──────────────────────────────────────────────────────
-- An invoice is now a header (billing_invoices) plus N lines. `kind`
-- distinguishes the fixed base charge from metered overage so the UI/report can
-- group them. quantity + unit_cents are informational; amount_cents is the
-- line's contribution to the invoice total.
CREATE TABLE IF NOT EXISTS billing_invoice_lines (
    id           UUID   PRIMARY KEY DEFAULT gen_random_uuid(),
    invoice_id   UUID   NOT NULL REFERENCES billing_invoices(id) ON DELETE CASCADE,
    tenant_id    UUID   NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    kind         TEXT   NOT NULL CHECK (kind IN ('base', 'seat_overage', 'storage_overage')),
    description  TEXT   NOT NULL,
    quantity     BIGINT NOT NULL DEFAULT 1 CHECK (quantity >= 0),
    unit_cents   BIGINT NOT NULL DEFAULT 0 CHECK (unit_cents >= 0),
    amount_cents BIGINT NOT NULL CHECK (amount_cents >= 0),
    -- One line per (invoice, kind): regenerating an invoice's lines is an upsert.
    UNIQUE (invoice_id, kind)
);

CREATE INDEX IF NOT EXISTS billing_invoice_lines_invoice_idx
    ON billing_invoice_lines (invoice_id);

ALTER TABLE billing_invoice_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing_invoice_lines FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS rls_billing_invoice_lines ON billing_invoice_lines;
CREATE POLICY rls_billing_invoice_lines ON billing_invoice_lines
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

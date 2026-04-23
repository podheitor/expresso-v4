-- Drop NOT NULL on audit_log.tenant_id.
-- Rationale: some audit events (failed logins, refresh failures, system events)
-- happen before a tenant context is established. The existing partial index
-- `audit_log_tenant_idx WHERE tenant_id IS NOT NULL` already assumes nullability.
ALTER TABLE audit_log ALTER COLUMN tenant_id DROP NOT NULL;

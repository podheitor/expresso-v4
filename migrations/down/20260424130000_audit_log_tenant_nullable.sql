-- DOWN: audit_log tenant nullable
-- The UP made tenant_id nullable; reverse by setting NOT NULL (only safe if no NULLs exist)
BEGIN;
-- Intentionally a no-op: enforcing NOT NULL here could break existing rows.
-- To fully reverse, run: UPDATE audit_log SET tenant_id = '00000000-0000-0000-0000-000000000000' WHERE tenant_id IS NULL;
-- Then: ALTER TABLE audit_log ALTER COLUMN tenant_id SET NOT NULL;
SELECT 1;
COMMIT;

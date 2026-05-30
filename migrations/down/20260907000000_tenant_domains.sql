-- DOWN: tenant domains
BEGIN;

DROP TRIGGER IF EXISTS tr_tenant_domains_touch ON tenant_domains;
DROP FUNCTION IF EXISTS tenant_domains_touch();

DROP POLICY IF EXISTS tenant_domains_rls ON tenant_domains;
ALTER TABLE tenant_domains DISABLE ROW LEVEL SECURITY;

DROP INDEX IF EXISTS idx_tenant_domains_tenant;
DROP TABLE IF EXISTS tenant_domains;

COMMIT;

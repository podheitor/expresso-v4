-- DOWN: core schema
BEGIN;

DROP TRIGGER IF EXISTS set_users_updated_at   ON users;
DROP TRIGGER IF EXISTS set_tenants_updated_at ON tenants;
DROP FUNCTION IF EXISTS set_updated_at();

DROP INDEX IF EXISTS idx_audit_action;
DROP INDEX IF EXISTS idx_audit_user;
DROP INDEX IF EXISTS idx_audit_tenant;
DROP TABLE IF EXISTS audit_log;

DROP POLICY IF EXISTS tenant_read_own        ON tenants;
DROP POLICY IF EXISTS tenant_isolation_users ON users;
ALTER TABLE users   DISABLE ROW LEVEL SECURITY;
ALTER TABLE tenants DISABLE ROW LEVEL SECURITY;

DROP ROLE IF EXISTS expresso_super_admin;

DROP INDEX IF EXISTS idx_users_cpf;
DROP INDEX IF EXISTS idx_users_email;
DROP INDEX IF EXISTS idx_users_tenant;
DROP TABLE IF EXISTS users;

DROP INDEX IF EXISTS idx_tenants_slug;
DROP TABLE IF EXISTS tenants;

DROP EXTENSION IF EXISTS "pg_trgm";
DROP EXTENSION IF EXISTS "uuid-ossp";

COMMIT;

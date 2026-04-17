-- Migration: core schema
-- Creates multi-tenant base tables with RLS

BEGIN;

-- Enable UUID extension
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pg_trgm";

-- ─── TENANTS ─────────────────────────────────────────────
CREATE TABLE tenants (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    slug        TEXT UNIQUE NOT NULL CHECK (slug ~ '^[a-z0-9]([a-z0-9-]*[a-z0-9])?$'),
    name        TEXT NOT NULL,
    cnpj        TEXT UNIQUE,
    plan        TEXT NOT NULL DEFAULT 'standard'
                CHECK (plan IN ('standard', 'professional', 'enterprise')),
    status      TEXT NOT NULL DEFAULT 'active'
                CHECK (status IN ('active', 'suspended', 'cancelled')),
    config      JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_tenants_slug ON tenants (slug);

-- ─── USERS ───────────────────────────────────────────────
CREATE TABLE users (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    email           TEXT NOT NULL,
    -- gov.br integration
    cpf             TEXT,
    sub_govbr       TEXT,
    govbr_acr       TEXT,       -- trust level: bronze/prata/ouro
    -- profile
    display_name    TEXT NOT NULL,
    given_name      TEXT,
    family_name     TEXT,
    avatar_path     TEXT,       -- S3 path
    preferred_lang  TEXT NOT NULL DEFAULT 'pt-BR',
    timezone        TEXT NOT NULL DEFAULT 'America/Sao_Paulo',
    -- quota
    quota_bytes     BIGINT NOT NULL DEFAULT 53687091200,  -- 50 GiB
    used_bytes      BIGINT NOT NULL DEFAULT 0,
    -- auth
    role            TEXT NOT NULL DEFAULT 'user'
                    CHECK (role IN ('super_admin', 'tenant_admin', 'helpdesk', 'user', 'readonly', 'guest')),
    is_active       BOOL NOT NULL DEFAULT true,
    mfa_enabled     BOOL NOT NULL DEFAULT false,
    last_login_at   TIMESTAMPTZ,
    -- metadata
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (tenant_id, email)
);

CREATE INDEX idx_users_tenant ON users (tenant_id);
CREATE INDEX idx_users_email  ON users (email);
CREATE INDEX idx_users_cpf    ON users (cpf) WHERE cpf IS NOT NULL;

-- ─── ROW LEVEL SECURITY ──────────────────────────────────
ALTER TABLE users   ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenants ENABLE ROW LEVEL SECURITY;

-- Tenant isolation: each request sets app.tenant_id
CREATE POLICY tenant_isolation_users ON users
    USING (tenant_id = current_setting('app.tenant_id', true)::UUID);

CREATE POLICY tenant_read_own ON tenants
    USING (id = current_setting('app.tenant_id', true)::UUID);

-- Super admins bypass RLS
CREATE ROLE expresso_super_admin;
ALTER TABLE users   FORCE ROW LEVEL SECURITY;
ALTER TABLE tenants FORCE ROW LEVEL SECURITY;

-- ─── AUDIT LOG (append-only) ─────────────────────────────
CREATE TABLE audit_log (
    id          BIGSERIAL PRIMARY KEY,
    tenant_id   UUID NOT NULL,
    user_id     UUID,           -- null for system actions
    action      TEXT NOT NULL,  -- e.g. "email.send", "auth.login"
    resource    TEXT,           -- e.g. "mailbox:uuid", "file:uuid"
    metadata    JSONB NOT NULL DEFAULT '{}',
    ip_addr     INET,
    user_agent  TEXT,
    status      TEXT NOT NULL DEFAULT 'success'
                CHECK (status IN ('success', 'failure', 'partial')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Append-only: revoke UPDATE/DELETE from all roles
REVOKE UPDATE, DELETE, TRUNCATE ON audit_log FROM PUBLIC;

CREATE INDEX idx_audit_tenant     ON audit_log (tenant_id, created_at DESC);
CREATE INDEX idx_audit_user       ON audit_log (user_id, created_at DESC) WHERE user_id IS NOT NULL;
CREATE INDEX idx_audit_action     ON audit_log (action, created_at DESC);

-- ─── TRIGGERS: updated_at ────────────────────────────────
CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER set_tenants_updated_at
    BEFORE UPDATE ON tenants
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER set_users_updated_at
    BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

COMMIT;

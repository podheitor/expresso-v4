-- Migration: contact groups (distribution lists)
-- A named, user-owned collection of contacts. Mirrors the addressbooks schema:
-- tenant_id FK, RLS via app.tenant_id, updated_at auto-bump. Membership is a
-- join table (group_id × contact_id) so a contact may belong to many groups.

BEGIN;

-- ─── CONTACT GROUPS ──────────────────────────────────────
CREATE TABLE IF NOT EXISTS contact_groups (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    owner_user_id   UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_contact_groups_tenant ON contact_groups (tenant_id);
CREATE INDEX IF NOT EXISTS idx_contact_groups_owner  ON contact_groups (owner_user_id);
-- A user's group names are unique (case-sensitive) to avoid silent dupes.
CREATE UNIQUE INDEX IF NOT EXISTS uq_contact_groups_owner_name
    ON contact_groups (owner_user_id, name);

-- ─── GROUP MEMBERSHIP ────────────────────────────────────
CREATE TABLE IF NOT EXISTS contact_group_members (
    group_id    UUID NOT NULL REFERENCES contact_groups (id) ON DELETE CASCADE,
    contact_id  UUID NOT NULL REFERENCES contacts (id) ON DELETE CASCADE,
    tenant_id   UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    added_at    TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (group_id, contact_id)
);

CREATE INDEX IF NOT EXISTS idx_group_members_contact ON contact_group_members (contact_id);
CREATE INDEX IF NOT EXISTS idx_group_members_tenant  ON contact_group_members (tenant_id);

-- ─── updated_at touch ────────────────────────────────────
CREATE OR REPLACE FUNCTION contact_groups_touch() RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS tr_contact_groups_touch ON contact_groups;
CREATE TRIGGER tr_contact_groups_touch
    BEFORE UPDATE ON contact_groups
    FOR EACH ROW EXECUTE FUNCTION contact_groups_touch();

-- Bump the parent group's updated_at whenever its membership changes, so a
-- listing's Last-Modified reflects member add/remove (not just renames).
CREATE OR REPLACE FUNCTION contact_group_members_touch() RETURNS TRIGGER AS $$
BEGIN
    UPDATE contact_groups
       SET updated_at = now()
     WHERE id = COALESCE(NEW.group_id, OLD.group_id);
    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS tr_contact_group_members_touch ON contact_group_members;
CREATE TRIGGER tr_contact_group_members_touch
    AFTER INSERT OR DELETE ON contact_group_members
    FOR EACH ROW EXECUTE FUNCTION contact_group_members_touch();

-- ─── RLS ─────────────────────────────────────────────────
-- Bootstrap-friendly: when app.tenant_id is NULL/empty, all rows visible
-- (migrations, superuser). Production MUST SET LOCAL app.tenant_id.
ALTER TABLE contact_groups        ENABLE ROW LEVEL SECURITY;
ALTER TABLE contact_group_members ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS contact_groups_rls ON contact_groups;
CREATE POLICY contact_groups_rls ON contact_groups
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR current_setting('app.tenant_id', true) = ''
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

DROP POLICY IF EXISTS contact_group_members_rls ON contact_group_members;
CREATE POLICY contact_group_members_rls ON contact_group_members
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR current_setting('app.tenant_id', true) = ''
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

COMMIT;

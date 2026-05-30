-- DOWN: contact groups
BEGIN;

DROP TRIGGER IF EXISTS tr_contact_group_members_touch ON contact_group_members;
DROP TRIGGER IF EXISTS tr_contact_groups_touch        ON contact_groups;
DROP FUNCTION IF EXISTS contact_group_members_touch();
DROP FUNCTION IF EXISTS contact_groups_touch();

DROP POLICY IF EXISTS contact_group_members_rls ON contact_group_members;
DROP POLICY IF EXISTS contact_groups_rls        ON contact_groups;
ALTER TABLE contact_group_members DISABLE ROW LEVEL SECURITY;
ALTER TABLE contact_groups        DISABLE ROW LEVEL SECURITY;

DROP INDEX IF EXISTS idx_group_members_tenant;
DROP INDEX IF EXISTS idx_group_members_contact;
DROP TABLE IF EXISTS contact_group_members;

DROP INDEX IF EXISTS uq_contact_groups_owner_name;
DROP INDEX IF EXISTS idx_contact_groups_owner;
DROP INDEX IF EXISTS idx_contact_groups_tenant;
DROP TABLE IF EXISTS contact_groups;

COMMIT;

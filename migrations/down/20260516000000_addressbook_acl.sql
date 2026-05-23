-- DOWN: addressbook ACL
BEGIN;
DROP INDEX IF EXISTS idx_addressbook_acl_tenant;
DROP INDEX IF EXISTS idx_addressbook_acl_grantee;
DROP TABLE IF EXISTS addressbook_acl;
COMMIT;

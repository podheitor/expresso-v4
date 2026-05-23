-- DOWN: contacts schema
BEGIN;

DROP TRIGGER IF EXISTS tr_contacts_touch     ON contacts;
DROP TRIGGER IF EXISTS tr_addressbooks_touch ON addressbooks;
DROP FUNCTION IF EXISTS contacts_touch();
DROP FUNCTION IF EXISTS addressbooks_touch();

DROP POLICY IF EXISTS contacts_rls     ON contacts;
DROP POLICY IF EXISTS addressbooks_rls ON addressbooks;
ALTER TABLE contacts     DISABLE ROW LEVEL SECURITY;
ALTER TABLE addressbooks DISABLE ROW LEVEL SECURITY;

DROP INDEX IF EXISTS uq_contacts_book_uid;
DROP INDEX IF EXISTS idx_contacts_fullname;
DROP INDEX IF EXISTS idx_contacts_email;
DROP INDEX IF EXISTS idx_contacts_tenant;
DROP INDEX IF EXISTS idx_contacts_book;
DROP TABLE IF EXISTS contacts;

DROP INDEX IF EXISTS uq_addressbooks_owner_default;
DROP INDEX IF EXISTS idx_addressbooks_owner;
DROP INDEX IF EXISTS idx_addressbooks_tenant;
DROP TABLE IF EXISTS addressbooks;

COMMIT;

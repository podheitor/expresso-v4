-- Revert: drop the denormalized BDAY/NICKNAME columns.

DROP INDEX IF EXISTS idx_contacts_nickname;
ALTER TABLE contacts
    DROP COLUMN IF EXISTS birthday,
    DROP COLUMN IF EXISTS nickname;

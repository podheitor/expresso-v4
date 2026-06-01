-- Denormalize vCard BDAY + NICKNAME onto contacts for indexing/filtering.
--
-- The full vCard already lives in contacts.vcard_raw; these columns surface two
-- common scalar fields the parser previously dropped, enabling birthday views
-- and nickname search without re-parsing the blob. birthday is stored as TEXT
-- (verbatim BDAY value) rather than DATE because RFC 6350 allows partial dates
-- (e.g. "--0515" for a recurring month-day with no year) that a DATE column
-- can't hold. NULL when the vCard omits the field.

ALTER TABLE contacts
    ADD COLUMN IF NOT EXISTS birthday TEXT,
    ADD COLUMN IF NOT EXISTS nickname TEXT;

-- Nickname participates in search alongside full_name/email.
CREATE INDEX IF NOT EXISTS idx_contacts_nickname ON contacts (nickname);

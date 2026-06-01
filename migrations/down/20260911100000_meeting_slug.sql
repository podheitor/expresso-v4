-- Revert: drop the meeting vanity slug.

DROP INDEX IF EXISTS uq_meetings_tenant_slug;
ALTER TABLE meetings DROP COLUMN IF EXISTS slug;

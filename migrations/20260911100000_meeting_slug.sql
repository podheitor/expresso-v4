-- Meeting vanity slug (human-readable join URL).
--
-- A meeting may carry an optional slug ("project-kickoff") so it can be joined
-- by a readable URL instead of its UUID. NULL = no slug (the default). Slugs are
-- unique per tenant (a partial unique index ignores NULLs, so many meetings can
-- stay slug-less). Resolution is tenant-scoped.

ALTER TABLE meetings
    ADD COLUMN IF NOT EXISTS slug TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS uq_meetings_tenant_slug
    ON meetings (tenant_id, slug) WHERE slug IS NOT NULL;

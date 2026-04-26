-- Add updated_at to govbr_user_map for precise Last-Modified caching.
-- Backfill with COALESCE(last_login_at, created_at) to match the previous
-- proxy value so existing ETags remain valid after migration.

BEGIN;

ALTER TABLE govbr_user_map
    ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

-- Backfill
UPDATE govbr_user_map
   SET updated_at = COALESCE(last_login_at, created_at);

-- Keep updated_at current on every upsert/update.
CREATE OR REPLACE FUNCTION govbr_user_map_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trig_govbr_user_map_updated_at ON govbr_user_map;
CREATE TRIGGER trig_govbr_user_map_updated_at
    BEFORE UPDATE ON govbr_user_map
    FOR EACH ROW EXECUTE FUNCTION govbr_user_map_set_updated_at();

COMMIT;

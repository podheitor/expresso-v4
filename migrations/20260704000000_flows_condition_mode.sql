-- Add condition_mode to flow_rules: "and" (default) or "or"
ALTER TABLE flow_rules
    ADD COLUMN IF NOT EXISTS condition_mode TEXT NOT NULL DEFAULT 'and';

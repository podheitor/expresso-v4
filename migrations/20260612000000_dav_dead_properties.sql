-- Dead-property store for CalDAV / CardDAV collections.
--
-- RFC 4918 §15 "dead" properties → arbitrary XML props PROPPATCH'd by clients
-- that servers MUST persist and return verbatim on PROPFIND. v1 scope: only
-- collection-level (calendar / addressbook); resource-level dead props
-- (events / contacts) can be added later by analogous tables.
--
-- Storage model:
--   * One FK per parent type → automatic CASCADE on parent delete.
--   * Unique (parent_id, namespace, local_name) → upsert key.
--   * xml_value = text content only (v1). Mixed XML can be added as raw-XML
--     column later without schema break.

CREATE TABLE IF NOT EXISTS calendar_dead_properties (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants (id)   ON DELETE CASCADE,
    calendar_id UUID        NOT NULL REFERENCES calendars (id) ON DELETE CASCADE,
    namespace   TEXT        NOT NULL,
    local_name  TEXT        NOT NULL,
    xml_value   TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT  uq_cal_dead_prop UNIQUE (calendar_id, namespace, local_name)
);

CREATE INDEX IF NOT EXISTS ix_cal_dead_prop_tenant
    ON calendar_dead_properties (tenant_id);

CREATE TABLE IF NOT EXISTS addressbook_dead_properties (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      UUID        NOT NULL REFERENCES tenants (id)     ON DELETE CASCADE,
    addressbook_id UUID        NOT NULL REFERENCES addressbooks (id) ON DELETE CASCADE,
    namespace      TEXT        NOT NULL,
    local_name     TEXT        NOT NULL,
    xml_value      TEXT        NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT     uq_ab_dead_prop UNIQUE (addressbook_id, namespace, local_name)
);

CREATE INDEX IF NOT EXISTS ix_ab_dead_prop_tenant
    ON addressbook_dead_properties (tenant_id);

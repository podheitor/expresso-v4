-- ActiveSync per-device, per-collection Sync state.
--
-- EAS Sync uses a rolling SyncKey per (device, collection=folder). We track the
-- current key plus the high-water UID already sent to that device for that
-- folder, so the next Sync emits only newer messages. Read-only mail sync first;
-- richer change tracking (flag/delete deltas) builds on this row.

BEGIN;

CREATE TABLE IF NOT EXISTS eas_sync_state (
    tenant_id     UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id       UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id     TEXT        NOT NULL,
    collection_id UUID        NOT NULL,
    sync_key      BIGINT      NOT NULL DEFAULT 0,
    last_uid      BIGINT      NOT NULL DEFAULT 0,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id, device_id, collection_id)
);

COMMIT;

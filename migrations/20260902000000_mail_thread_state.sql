-- Migration: per-user conversation state (mute / pin).
-- A sparse overlay on threads: a row exists only while at least one flag is
-- true (the API deletes it when both return to false), so most threads have no
-- row and list_threads LEFT JOINs to read them as {muted:false, pinned:false}.
-- Keyed by thread_id (not message id) so state survives message deletion and
-- applies to the whole conversation.

BEGIN;

CREATE TABLE IF NOT EXISTS mail_thread_state (
    tenant_id   UUID        NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    user_id     UUID        NOT NULL REFERENCES users (id)   ON DELETE CASCADE,
    thread_id   UUID        NOT NULL,
    muted       BOOLEAN     NOT NULL DEFAULT false,
    pinned      BOOLEAN     NOT NULL DEFAULT false,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id, thread_id)
);

-- Supports the pinned-first ORDER BY and per-user lookups in list_threads.
CREATE INDEX IF NOT EXISTS mail_thread_state_pinned_idx
    ON mail_thread_state (tenant_id, user_id)
    WHERE pinned;

COMMIT;

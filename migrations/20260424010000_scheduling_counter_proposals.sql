-- COUNTER proposals (iTIP RFC 5546 §3.2.7) — persisted until organizer decides.
--
-- Status flow:  pending → accepted  (event updated, SEQUENCE auto-bumps)
--            →  pending → rejected  (DECLINECOUNTER sent by admin; event unchanged)

CREATE TABLE IF NOT EXISTS scheduling_counter_proposals (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id         UUID NOT NULL,
    event_id          UUID NOT NULL REFERENCES calendar_events(id) ON DELETE CASCADE,
    attendee_email    TEXT NOT NULL,
    proposed_dtstart  TIMESTAMPTZ,
    proposed_dtend    TIMESTAMPTZ,
    comment           TEXT,
    status            TEXT NOT NULL DEFAULT 'pending'
                      CHECK (status IN ('pending','accepted','rejected')),
    received_sequence INT,
    raw_ical          TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at       TIMESTAMPTZ,
    resolved_by       UUID
);

CREATE INDEX IF NOT EXISTS idx_counter_prop_tenant_status
    ON scheduling_counter_proposals(tenant_id, status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_counter_prop_event
    ON scheduling_counter_proposals(event_id, status);

COMMENT ON TABLE scheduling_counter_proposals IS
  'iTIP COUNTER proposals awaiting organizer decision (RFC 5546 §3.2.7).';

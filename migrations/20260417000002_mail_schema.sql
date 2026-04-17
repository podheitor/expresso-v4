-- Migration: email/mailbox schema

BEGIN;

-- ─── MAILBOXES ───────────────────────────────────────────
CREATE TABLE mailboxes (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    tenant_id    UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    -- IMAP folder attributes
    folder_name  TEXT NOT NULL,        -- "INBOX", "Sent", "Drafts", "$Label1", ...
    uid_validity BIGINT NOT NULL DEFAULT EXTRACT(EPOCH FROM now())::BIGINT,
    next_uid     BIGINT NOT NULL DEFAULT 1,
    -- IANA special-use attributes (RFC 6154)
    special_use  TEXT,                 -- "\Inbox" "\Sent" "\Drafts" "\Trash" "\Junk"
    subscribed   BOOL NOT NULL DEFAULT true,
    -- stats (kept in sync via trigger)
    message_count   INT NOT NULL DEFAULT 0,
    unseen_count    INT NOT NULL DEFAULT 0,
    size_bytes      BIGINT NOT NULL DEFAULT 0,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (user_id, folder_name)
);

CREATE INDEX idx_mailboxes_user ON mailboxes (user_id);

-- ─── MESSAGES ────────────────────────────────────────────
CREATE TABLE messages (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    mailbox_id   UUID NOT NULL REFERENCES mailboxes (id) ON DELETE CASCADE,
    tenant_id    UUID NOT NULL REFERENCES tenants (id),
    -- IMAP metadata
    uid          BIGINT NOT NULL,
    flags        TEXT[] NOT NULL DEFAULT '{}',    -- \Seen \Answered \Flagged \Deleted
    -- Envelope
    subject      TEXT,
    from_addr    TEXT,
    from_name    TEXT,
    to_addrs     JSONB NOT NULL DEFAULT '[]',     -- [{addr, name}]
    cc_addrs     JSONB NOT NULL DEFAULT '[]',
    bcc_addrs    JSONB NOT NULL DEFAULT '[]',
    reply_to     TEXT,
    -- Threading
    message_id   TEXT,
    in_reply_to  TEXT,
    references_  TEXT[],
    thread_id    UUID,                           -- computed on ingest
    -- Body info
    has_attachments BOOL NOT NULL DEFAULT false,
    size_bytes   INT NOT NULL DEFAULT 0,
    -- Storage: body stored in MinIO
    body_path    TEXT NOT NULL,                  -- s3://bucket/tenant_id/user_id/msg_id.eml.zst
    preview_text TEXT,                           -- first 500 chars of plain text
    -- Timing
    date         TIMESTAMPTZ,                    -- Date header
    received_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (mailbox_id, uid)
);

CREATE INDEX idx_messages_mailbox      ON messages (mailbox_id, received_at DESC);
CREATE INDEX idx_messages_thread       ON messages (thread_id) WHERE thread_id IS NOT NULL;
CREATE INDEX idx_messages_message_id   ON messages (message_id) WHERE message_id IS NOT NULL;
CREATE INDEX idx_messages_flags        ON messages USING GIN (flags);
-- Full-text search (supplement to Tantivy)
CREATE INDEX idx_messages_subject_fts  ON messages USING GIN (to_tsvector('portuguese', coalesce(subject, '')));

-- ─── ALIASES ─────────────────────────────────────────────
CREATE TABLE email_aliases (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    alias       TEXT NOT NULL,              -- alias address
    target      TEXT NOT NULL,              -- canonical address or external
    is_enabled  BOOL NOT NULL DEFAULT true,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (tenant_id, alias)
);

-- ─── DKIM KEYS ───────────────────────────────────────────
CREATE TABLE dkim_keys (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    domain      TEXT NOT NULL,
    selector    TEXT NOT NULL DEFAULT 'expresso',
    private_key TEXT NOT NULL,   -- encrypted at rest (age)
    public_key  TEXT NOT NULL,
    algorithm   TEXT NOT NULL DEFAULT 'rsa-sha256',
    is_active   BOOL NOT NULL DEFAULT true,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ,

    UNIQUE (tenant_id, domain, selector)
);

-- RLS for mail tables
ALTER TABLE mailboxes     ENABLE ROW LEVEL SECURITY;
ALTER TABLE messages      ENABLE ROW LEVEL SECURITY;
ALTER TABLE email_aliases ENABLE ROW LEVEL SECURITY;
ALTER TABLE dkim_keys     ENABLE ROW LEVEL SECURITY;

CREATE POLICY rls_mailboxes     ON mailboxes     USING (tenant_id = current_setting('app.tenant_id', true)::UUID);
CREATE POLICY rls_messages      ON messages      USING (tenant_id = current_setting('app.tenant_id', true)::UUID);
CREATE POLICY rls_email_aliases ON email_aliases USING (tenant_id = current_setting('app.tenant_id', true)::UUID);
CREATE POLICY rls_dkim_keys     ON dkim_keys     USING (tenant_id = current_setting('app.tenant_id', true)::UUID);

COMMIT;

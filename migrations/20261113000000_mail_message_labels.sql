-- Migration: per-user mail labels (Outlook categories).
-- A user tags a message with one or more named labels (a small preset set
-- lives client-side: importante/trabalho/pessoal/aguardando). Labels are a
-- personal overlay — they never touch the shared message row. The label name
-- is stored verbatim; (tenant,user,message,label) is unique.

BEGIN;

CREATE TABLE IF NOT EXISTS mail_message_labels (
    tenant_id   UUID        NOT NULL,
    user_id     UUID        NOT NULL,
    message_id  UUID        NOT NULL,
    label       TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id, message_id, label)
);

-- Look up all labels for a set of messages (the inbox render path).
CREATE INDEX IF NOT EXISTS mail_message_labels_msg_idx
    ON mail_message_labels (tenant_id, user_id, message_id);

COMMIT;

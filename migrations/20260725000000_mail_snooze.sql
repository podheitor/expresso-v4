-- Sprint #409: mail message snooze
CREATE TABLE IF NOT EXISTS mail_snooze (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID NOT NULL,
    user_id      UUID NOT NULL,
    message_uid  BIGINT NOT NULL,
    mailbox_id   UUID NOT NULL,
    snooze_until TIMESTAMPTZ NOT NULL,
    snoozed_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    woken_at     TIMESTAMPTZ,
    UNIQUE (tenant_id, user_id, message_uid, mailbox_id)
);

CREATE INDEX IF NOT EXISTS mail_snooze_due_idx
    ON mail_snooze (snooze_until)
    WHERE woken_at IS NULL;

CREATE INDEX IF NOT EXISTS mail_snooze_user_idx
    ON mail_snooze (tenant_id, user_id, woken_at);

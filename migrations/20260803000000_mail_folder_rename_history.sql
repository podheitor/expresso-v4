-- Sprint #480: mail folder rename history
CREATE TABLE IF NOT EXISTS mail_folder_rename_history (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL,
    user_id         UUID NOT NULL,
    mailbox_id      UUID NOT NULL,
    old_name        TEXT NOT NULL,
    new_name        TEXT NOT NULL,
    renamed_by      UUID NOT NULL,
    renamed_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS mail_folder_rename_history_tenant_user_at_idx
    ON mail_folder_rename_history (tenant_id, user_id, renamed_at DESC);

CREATE INDEX IF NOT EXISTS mail_folder_rename_history_mailbox_idx
    ON mail_folder_rename_history (tenant_id, mailbox_id);

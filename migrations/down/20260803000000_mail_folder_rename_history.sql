-- DOWN: mail folder rename history
BEGIN;
DROP INDEX IF EXISTS mail_folder_rename_history_mailbox_idx;
DROP INDEX IF EXISTS mail_folder_rename_history_tenant_user_at_idx;
DROP TABLE IF EXISTS mail_folder_rename_history;
COMMIT;

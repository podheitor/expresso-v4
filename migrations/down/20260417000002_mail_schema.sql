-- DOWN: mail schema
BEGIN;

ALTER TABLE dkim_keys     DISABLE ROW LEVEL SECURITY;
ALTER TABLE email_aliases DISABLE ROW LEVEL SECURITY;
ALTER TABLE messages      DISABLE ROW LEVEL SECURITY;
ALTER TABLE mailboxes     DISABLE ROW LEVEL SECURITY;

DROP TABLE IF EXISTS dkim_keys;
DROP TABLE IF EXISTS email_aliases;

DROP INDEX IF EXISTS idx_messages_subject_fts;
DROP INDEX IF EXISTS idx_messages_flags;
DROP INDEX IF EXISTS idx_messages_message_id;
DROP INDEX IF EXISTS idx_messages_thread;
DROP INDEX IF EXISTS idx_messages_mailbox;
DROP TABLE IF EXISTS messages;

DROP INDEX IF EXISTS idx_mailboxes_user;
DROP TABLE IF EXISTS mailboxes;

COMMIT;

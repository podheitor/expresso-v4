-- DOWN: imap mailbox subscribed index
BEGIN;
DROP INDEX IF EXISTS idx_mailboxes_subscribed;
COMMIT;

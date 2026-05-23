-- DOWN: notifications table
BEGIN;
DROP INDEX IF EXISTS notifications_unread_idx;
DROP INDEX IF EXISTS notifications_user_idx;
DROP TABLE IF EXISTS notifications;
COMMIT;

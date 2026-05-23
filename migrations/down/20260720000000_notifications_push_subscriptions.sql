-- DOWN: notification push subscriptions
BEGIN;
DROP INDEX IF EXISTS notification_push_subscriptions_user_idx;
DROP TABLE IF EXISTS notification_push_subscriptions;
COMMIT;

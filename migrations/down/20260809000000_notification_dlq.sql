-- DOWN: notification DLQ
BEGIN;
DROP INDEX IF EXISTS notification_dlq_failed_at;
DROP TABLE IF EXISTS notification_dlq;
COMMIT;

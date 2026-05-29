-- DOWN: per-user conversation state (mute / pin)
BEGIN;
DROP INDEX IF EXISTS mail_thread_state_pinned_idx;
DROP TABLE IF EXISTS mail_thread_state;
COMMIT;

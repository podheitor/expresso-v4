-- DOWN: drive comment reactions
BEGIN;
DROP INDEX IF EXISTS drive_comment_reactions_comment_idx;
DROP TABLE IF EXISTS drive_comment_reactions;
COMMIT;

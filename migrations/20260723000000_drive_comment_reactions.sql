-- Sprint #406: drive file comment reactions
CREATE TABLE IF NOT EXISTS drive_comment_reactions (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    comment_id  UUID NOT NULL,
    file_id     UUID NOT NULL,
    tenant_id   UUID NOT NULL,
    user_id     UUID NOT NULL,
    emoji       TEXT NOT NULL CHECK (char_length(emoji) BETWEEN 1 AND 8),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (comment_id, tenant_id, user_id, emoji)
);

CREATE INDEX IF NOT EXISTS drive_comment_reactions_comment_idx
    ON drive_comment_reactions (tenant_id, comment_id);

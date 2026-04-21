BEGIN;

-- Allow bootstrap/service execution when app.tenant_id is not set yet.
DROP POLICY IF EXISTS tenant_isolation_users ON users;
CREATE POLICY tenant_isolation_users ON users
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

DROP POLICY IF EXISTS tenant_read_own ON tenants;
CREATE POLICY tenant_read_own ON tenants
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR id = current_setting('app.tenant_id', true)::UUID
    );

DROP POLICY IF EXISTS rls_mailboxes ON mailboxes;
CREATE POLICY rls_mailboxes ON mailboxes
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

DROP POLICY IF EXISTS rls_messages ON messages;
CREATE POLICY rls_messages ON messages
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

DROP POLICY IF EXISTS rls_email_aliases ON email_aliases;
CREATE POLICY rls_email_aliases ON email_aliases
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

DROP POLICY IF EXISTS rls_dkim_keys ON dkim_keys;
CREATE POLICY rls_dkim_keys ON dkim_keys
    USING (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    )
    WITH CHECK (
        current_setting('app.tenant_id', true) IS NULL
        OR tenant_id = current_setting('app.tenant_id', true)::UUID
    );

-- Keep mailbox counters synchronized from messages table.
CREATE OR REPLACE FUNCTION refresh_mailbox_stats(target_mailbox_id UUID)
RETURNS VOID AS $$
BEGIN
    UPDATE mailboxes mb
    SET
        message_count = COALESCE(stats.message_count, 0),
        unseen_count  = COALESCE(stats.unseen_count, 0),
        size_bytes    = COALESCE(stats.size_bytes, 0),
        next_uid      = COALESCE(stats.next_uid, 1),
        updated_at    = now()
    FROM (
        SELECT
            mailbox_id,
            COUNT(*)::INT AS message_count,
            COUNT(*) FILTER (WHERE NOT (E'\\Seen' = ANY(flags)))::INT AS unseen_count,
            COALESCE(SUM(size_bytes), 0)::BIGINT AS size_bytes,
            COALESCE(MAX(uid) + 1, 1)::BIGINT AS next_uid
        FROM messages
        WHERE mailbox_id = target_mailbox_id
        GROUP BY mailbox_id
    ) AS stats
    WHERE mb.id = target_mailbox_id;

    UPDATE mailboxes
    SET
        message_count = 0,
        unseen_count  = 0,
        size_bytes    = 0,
        next_uid      = 1,
        updated_at    = now()
    WHERE id = target_mailbox_id
      AND NOT EXISTS (SELECT 1 FROM messages WHERE mailbox_id = target_mailbox_id);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION messages_sync_mailbox_stats()
RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        PERFORM refresh_mailbox_stats(NEW.mailbox_id);
    ELSIF TG_OP = 'DELETE' THEN
        PERFORM refresh_mailbox_stats(OLD.mailbox_id);
    ELSE
        PERFORM refresh_mailbox_stats(NEW.mailbox_id);
        IF NEW.mailbox_id <> OLD.mailbox_id THEN
            PERFORM refresh_mailbox_stats(OLD.mailbox_id);
        END IF;
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_messages_sync_mailbox_stats ON messages;
CREATE TRIGGER trg_messages_sync_mailbox_stats
AFTER INSERT OR UPDATE OF mailbox_id, flags, size_bytes, uid OR DELETE
ON messages
FOR EACH ROW
EXECUTE FUNCTION messages_sync_mailbox_stats();

-- Backfill counters for existing rows.
UPDATE mailboxes mb
SET
    message_count = COALESCE(stats.message_count, 0),
    unseen_count  = COALESCE(stats.unseen_count, 0),
    size_bytes    = COALESCE(stats.size_bytes, 0),
    next_uid      = COALESCE(stats.next_uid, 1),
    updated_at    = now()
FROM (
    SELECT
        mailbox_id,
        COUNT(*)::INT AS message_count,
        COUNT(*) FILTER (WHERE NOT (E'\\Seen' = ANY(flags)))::INT AS unseen_count,
        COALESCE(SUM(size_bytes), 0)::BIGINT AS size_bytes,
        COALESCE(MAX(uid) + 1, 1)::BIGINT AS next_uid
    FROM messages
    GROUP BY mailbox_id
) AS stats
WHERE mb.id = stats.mailbox_id;

UPDATE mailboxes mb
SET
    message_count = 0,
    unseen_count  = 0,
    size_bytes    = 0,
    next_uid      = 1,
    updated_at    = now()
WHERE NOT EXISTS (SELECT 1 FROM messages m WHERE m.mailbox_id = mb.id);

COMMIT;

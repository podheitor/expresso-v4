-- DOWN: rls bootstrap and mailbox stats
BEGIN;

DROP TRIGGER IF EXISTS trg_messages_sync_mailbox_stats ON messages;
DROP FUNCTION IF EXISTS messages_sync_mailbox_stats();
DROP FUNCTION IF EXISTS refresh_mailbox_stats(UUID);

-- Restore original (permissive) RLS policies dropped by this migration
CREATE POLICY tenant_isolation_users ON users
    USING (tenant_id = current_setting('app.tenant_id', true)::UUID);

CREATE POLICY tenant_read_own ON tenants
    USING (id = current_setting('app.tenant_id', true)::UUID);

CREATE POLICY rls_mailboxes ON mailboxes
    USING (tenant_id = current_setting('app.tenant_id', true)::UUID);

CREATE POLICY rls_messages ON messages
    USING (tenant_id = current_setting('app.tenant_id', true)::UUID);

CREATE POLICY rls_email_aliases ON email_aliases
    USING (tenant_id = current_setting('app.tenant_id', true)::UUID);

CREATE POLICY rls_dkim_keys ON dkim_keys
    USING (tenant_id = current_setting('app.tenant_id', true)::UUID);

COMMIT;
